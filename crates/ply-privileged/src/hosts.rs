use ply_compiled::entry::Grants;
use ply_core::host::HostBinding;
use ply_core::{Diagnostic, Span};
use ply_host::config::{Entry, Snapshot};
use ply_host::tls;
use std::sync::Arc;

pub struct Hosts {
    host: Arc<ply_host::Host>,
    binding: Arc<HostBinding>,
    stop: Option<Arc<ply_host::signal::Shutdown>>,
}

impl Hosts {
    pub fn granted(grants: &Grants) -> Result<Hosts, Vec<Diagnostic>> {
        let pairs = |name: &'static str| {
            grants.named(name).filter_map(|fields| match fields {
                [name, path] => Some((name.clone(), std::path::PathBuf::from(path))),
                _ => None,
            })
        };
        let draining = grants.named("drain").next().map(|fields| {
            let ms = |at: usize, default: u64| {
                let ms = fields.get(at).and_then(|f| f.parse::<u64>().ok());
                std::time::Duration::from_millis(ms.unwrap_or(default))
            };
            ply_host::signal::Shutdown::new(ply_host::signal::Bounds {
                lead: ms(0, ply_host::signal::DEFAULT_STOP_DELAY_MS),
                drain: ms(1, ply_host::signal::DEFAULT_DRAIN_MS),
            })
        });
        let stop = draining.or_else(|| {
            grants
                .has("end")
                .then(ply_host::signal::Shutdown::without_drain)
        });
        if let Some(stop) = &stop {
            ply_host::signal::listen(stop).map_err(|d| vec![d])?;
        }
        let credentials: Vec<tls::CredentialSpec> = grants
            .named("tls")
            .filter_map(|fields| match fields {
                [name, certificate, key] => Some(tls::CredentialSpec {
                    name: name.clone(),
                    certificate: certificate.into(),
                    key: key.into(),
                }),
                _ => None,
            })
            .collect();
        let first = |name: &'static str| {
            grants
                .named(name)
                .filter_map(|fields| fields.first().cloned())
                .collect::<Vec<String>>()
        };
        let trust: Vec<std::path::PathBuf> = first("trust").into_iter().map(Into::into).collect();
        let material = tls::Credentials::load(&credentials, &trust, &first("mtls"))?;
        let roots: Vec<ply_host::fs::RootSpec> = pairs("fs")
            .map(|(name, path)| ply_host::fs::RootSpec { name, path })
            .collect();
        let roots = ply_host::fs::Roots::load(&roots, Span::DUMMY).map_err(|d| vec![d])?;
        let programs: Vec<ply_host::process::ExecSpec> = pairs("exec")
            .map(|(name, path)| ply_host::process::ExecSpec { name, path })
            .collect();
        let executables =
            ply_host::process::Executables::load(&programs, Span::DUMMY).map_err(|d| vec![d])?;
        let process = if grants.has("spawning") {
            Some(ply_host::process::ProcessHost::spawning(executables))
        } else {
            grants.named("argv").next().map(|argv| {
                let out = if grants.has("stderr") {
                    ply_host::process::Stream::Err
                } else {
                    ply_host::process::Stream::Out
                };
                ply_host::process::ProcessHost::new(
                    argv.to_vec(),
                    ply_host::process::OutputSink::Real { out },
                )
                .executing(executables)
            })
        };
        let mut facilities = ply_host::Host::with_credentials(material)
            .configured(configured(grants))
            .rooted(roots)
            .traced(crate::trace::TraceOptions::of_grants(grants).open());
        if let Some(process) = process {
            facilities = facilities.with_process(process);
        }
        if let Some(stop) = &stop {
            facilities = facilities.stopping_on(Arc::clone(stop));
        }
        Ok(Hosts {
            host: Arc::new(facilities),
            binding: Arc::new(HostBinding::granted(grants.clone())),
            stop,
        })
    }

    pub fn shutdown_reports(&self) -> Vec<(String, Vec<u8>)> {
        use ply_compiled::flat::Writer;
        let mut reports = Vec::new();
        let mut report = |name: &str, write: &dyn Fn(&mut Writer)| {
            let mut w = Writer::new();
            write(&mut w);
            reports.push((name.to_string(), w.finish()));
        };
        let torn = self.host.runtime().shutdown();
        report("teardown", &|w| {
            w.count(torn.spans_left_open as u64);
        });
        let handshakes = self.host.handshakes();
        if !handshakes.is_empty() {
            report("handshakes", &|w| {
                w.count(handshakes.completed)
                    .count(handshakes.rejected)
                    .list(handshakes.reasons.iter(), |w, (reason, n)| {
                        w.str(reason).count(*n);
                    });
            });
        }
        if let Some(stop) = self.stop.as_ref().filter(|s| s.stopping()) {
            let (listeners, connections) = stop.stop_counts();
            report("stopping", &|w| {
                w.option(stop.signal(), |w, s| {
                    w.str(s.name());
                })
                .count(listeners as u64)
                .count(connections as u64)
                .count(stop.elapsed().unwrap_or_default().as_millis() as u64);
            });
        }
        let counts = self.host.tracing().counts();
        report("trace", &|w| {
            w.count(counts.events)
                .count(counts.spans)
                .count(counts.abandoned)
                .bool(counts.flushed);
        });
        reports
    }

    pub fn runtime_factory(&self) -> ply_core::RuntimeFactory {
        let host = Arc::clone(&self.host);
        Arc::new(move || host.runtime())
    }

    pub fn binding(&self) -> Arc<HostBinding> {
        Arc::clone(&self.binding)
    }

    pub fn requested_exit(&self) -> Option<i32> {
        self.host.process()?.requested_exit()
    }
}

impl ply_compiled::entry::Bound for Hosts {
    fn binding(&self) -> Arc<HostBinding> {
        Hosts::binding(self)
    }

    fn runtime_factory(&self) -> ply_core::RuntimeFactory {
        Hosts::runtime_factory(self)
    }

    fn requested_exit(&self) -> Option<i32> {
        Hosts::requested_exit(self)
    }

    fn traced(&self, _: &ply_host::observe::Recorder, _: bool) -> Option<String> {
        None
    }

    fn unbind(&self) -> Vec<(String, Vec<u8>)> {
        self.shutdown_reports()
    }
}

fn configured(grants: &Grants) -> Arc<Snapshot> {
    let values = grants
        .named("config")
        .filter_map(|fields| match fields {
            [key, value, secret] => Some((
                key.clone(),
                Entry {
                    value: value.clone(),
                    secret: secret == "secret",
                },
            )),
            _ => None,
        })
        .collect();
    Arc::new(Snapshot::new(values, grants.has("schema")))
}
