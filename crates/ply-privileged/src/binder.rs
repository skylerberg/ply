use crate::hosts::Hosts;
use ply_compiled::entry::{Bound, Grants};
use ply_compiled::flat::Writer;
use ply_core::Diagnostic;
use ply_core::host::HostBinding;
use ply_host::observe::Recorder;
use ply_host::tls;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

pub fn install() {
    ply_compiled::entry::install(Box::new(Binder));
    crate::shipped::install();
}

struct Binder;

impl ply_compiled::entry::Binder for Binder {
    fn bind(&self, grants: Grants) -> Result<Arc<dyn Bound>, Vec<Diagnostic>> {
        let hosts = Hosts::granted(&grants)?;
        Ok(Arc::new(Borrowed {
            world: GrantedContext::of(&grants),
            hosts,
        }))
    }

    fn hosting(&self, grants: &Grants, w: &mut Writer) {
        let mut rejections = Vec::new();
        let credentials = credentials_of(grants)
            .map_err(|d| rejections.extend(d))
            .ok();
        let roots = roots_of(grants).map_err(|d| rejections.push(d)).ok();
        w.list(rejections.iter(), |w, d| {
            ply_compiled::entry::write_failure(w, d, None)
        });
        w.option(credentials.as_ref(), |w, c| {
            w.str(tls::LIBRARY)
                .str(tls::VERSION)
                .str(tls::PROVIDER)
                .list(tls::VERSIONS.iter(), |w, v| {
                    w.str(v);
                })
                .list(tls::ALPN.iter(), |w, a| {
                    w.str(a);
                })
                .str(tls::ROOT_CERTIFICATES_LIBRARY)
                .str(tls::ROOTS_VERSION)
                .count(c.trusted() as u64)
                .list(
                    c.iter().collect::<Vec<_>>().into_iter(),
                    |w, (name, credential)| {
                        w.str(name)
                            .str(credential.fingerprint())
                            .count(credential.certificates() as u64);
                    },
                );
        });
        let listed: Vec<(String, String)> = (roots.iter())
            .flat_map(|r| r.listing())
            .map(|(name, path)| (name.to_string(), path.display().to_string()))
            .collect();
        w.list(listed.iter(), |w, (name, path)| {
            w.str(name).str(path);
        });
        let trace = crate::trace::TraceOptions::of_grants(grants);
        let opened = trace.open();
        let sink = opened.sink_module_path();
        w.str(sink).str(opened.sink_destination_name()).option(
            (sink != ply_host::trace::DISCARD_PATH).then_some(trace.level_name()),
            |w, level| {
                w.str(level);
            },
        );
        let listening = ply_host::signal::Shutdown::new(ply_host::signal::Bounds::default());
        w.list(listening.signals().iter(), |w, s| {
            w.str(s.name());
        });
    }

    fn moved(&self, grants: &Grants, trace: &str, caller: Option<&Recorder>) -> Option<String> {
        ply_host::observe::moved(trace, &GrantedContext::of(grants).world(), caller)
    }
}

fn credentials_of(grants: &Grants) -> Result<tls::Credentials, Vec<Diagnostic>> {
    let named: Vec<tls::CredentialSpec> = grants
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
        (grants.named(name))
            .filter_map(|fields| fields.first().cloned())
            .collect::<Vec<String>>()
    };
    let trust: Vec<PathBuf> = first("trust").into_iter().map(Into::into).collect();
    tls::Credentials::load(&named, &trust, &first("mtls"))
}

fn roots_of(grants: &Grants) -> Result<ply_host::fs::Roots, Diagnostic> {
    let specs: Vec<ply_host::fs::RootSpec> = grants
        .named("fs")
        .filter_map(|fields| match fields {
            [name, path] => Some(ply_host::fs::RootSpec {
                name: name.clone(),
                path: path.into(),
            }),
            _ => None,
        })
        .collect();
    ply_host::fs::Roots::load(&specs, ply_core::Span::DUMMY)
}

struct Borrowed {
    hosts: Hosts,
    world: GrantedContext,
}

impl Bound for Borrowed {
    fn binding(&self) -> Arc<HostBinding> {
        self.hosts.binding()
    }

    fn runtime_factory(&self) -> ply_core::RuntimeFactory {
        self.hosts.runtime_factory()
    }

    fn requested_exit(&self) -> Option<i32> {
        self.hosts.requested_exit()
    }

    fn traced(&self, recorder: &Recorder, host: bool) -> Option<String> {
        ply_host::observe::finished(recorder, &self.world.world(), host)
    }

    fn unbind(&self) -> Vec<(String, Vec<u8>)> {
        self.hosts.shutdown_reports()
    }
}

#[derive(Clone)]
struct GrantedContext {
    roots: Vec<(String, PathBuf)>,
    binding: String,
    config: BTreeMap<String, String>,
}

impl GrantedContext {
    fn of(grants: &Grants) -> GrantedContext {
        let roots = roots_of(grants)
            .map(|roots| {
                roots
                    .listing()
                    .map(|(name, dir)| (name.to_string(), dir.to_path_buf()))
                    .collect()
            })
            .unwrap_or_default();
        let config = (grants.named("config"))
            .filter_map(|fields| match fields {
                [key, value, kind] if kind == "plain" => Some((key.clone(), value.clone())),
                _ => None,
            })
            .collect();
        GrantedContext {
            roots,
            binding: binding_digest(grants),
            config,
        }
    }

    fn world(&self) -> ply_host::observe::World<'_> {
        ply_host::observe::World {
            roots: &self.roots,
            binding: self.binding.clone(),
            config: &self.config,
            binary: ply_host::observe::Binary {
                shipped: &crate::shipped::module_digest,
                program: crate::shipped::program_digest(),
            },
        }
    }
}

fn binding_digest(grants: &Grants) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in ["fs", "exec", "allow"] {
        let mut names: Vec<&str> = (grants.named(part))
            .filter_map(|fields| fields.first().map(String::as_str))
            .collect();
        names.sort();
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
        for name in names {
            hasher.update(name.as_bytes());
            hasher.update(&[0]);
        }
    }
    hasher.finalize().to_hex().to_string()
}
