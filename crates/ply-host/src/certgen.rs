//! Certificates a run generates for itself: throwaway localhost credentials for benches and
//! tests, beside the TLS code that loads the ones a run is given.

use crate::tls;
use ply_eval::host::{
    Determinism, HostAnswer, HostHandler, HostOp, HostRegistry, HostRequest, HostResource,
    Linearity,
};
use ply_eval::{Diagnostic, Symbol, Value, codes};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const EFFECT: &str = "certgen";

operations! {
    what "certgen";
    path "certgen";
    Issue = "issue" / 0,
}

impl Op {
    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            // A fresh key is not a function of program state, and nothing may be shared between
            // two runs of the same program.
            determinism: Determinism::Nondeterministic,
            linearity: Linearity::Repeatable,
            blocking: false,
            secrets: false,
            path: self.path(),
        }
    }
}

/// One generated certificate: what a server is given, and what a client trusts.
pub struct Issued {
    /// The certificate, PEM-encoded.
    pub certificate: String,
    /// Its private key, PEM-encoded.
    pub key: String,
    /// The certificate's DER bytes, which is what a client's root store wants.
    pub der: Vec<u8>,
    /// SHA-256 of the DER, lowercase hex.
    pub fingerprint: String,
}

/// A self-signed certificate for `names`, valid immediately. Nothing checked in is trusted by
/// anything, and each call makes a new key.
pub fn issue(names: &[String]) -> Result<Issued, String> {
    let names: Vec<String> = if names.is_empty() {
        vec!["localhost".to_string()]
    } else {
        names.to_vec()
    };
    let issued =
        rcgen::generate_simple_self_signed(names).map_err(|e| format!("generating: {e}"))?;
    let der = issued.cert.der().to_vec();
    Ok(Issued {
        certificate: issued.cert.pem(),
        key: issued.signing_key.serialize_pem(),
        fingerprint: tls::fingerprint(issued.cert.der()),
        der,
    })
}

pub fn registrations() -> Vec<(HostOp, Arc<dyn HostHandler>)> {
    Op::ALL
        .iter()
        .map(|op| {
            let handler: Arc<dyn HostHandler> = Arc::new(Operation { op: *op });
            (op.declaration(), handler)
        })
        .collect()
}

pub fn register(registry: &mut HostRegistry) {
    for (op, handler) in registrations() {
        registry.register(op, handler);
    }
}

struct Operation {
    op: Op,
}

impl HostHandler for Operation {
    fn call(
        &self,
        _: &dyn ply_eval::host::HostRuntime,
        req: &HostRequest<'_>,
    ) -> Result<HostAnswer, Diagnostic> {
        if req.args.len() != self.op.arity() {
            return Err(arity(self.op, req.args.len(), req.span));
        }
        Ok(HostAnswer::Value(match self.op {
            Op::Issue => {
                let issued = issue(&[]).map_err(|why| {
                    Diagnostic::error(codes::RUNTIME_ERROR, why)
                        .primary(req.span, "`certgen.issue` could not generate a certificate")
                })?;
                issued_value(&issued)
            }
        }))
    }
}

#[cold]
fn arity(op: Op, got: usize, span: ply_eval::Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "{} was performed with {got} argument(s) and takes {}",
            op.what(),
            op.arity()
        ),
    )
    .primary(span, "this perform reached the host handler")
    .note("inference checks a perform's arity, so reaching this means the evaluator was handed a module that was never checked")
}

/// A generated certificate as the program reads it: `std.certgen`'s `Issued`.
pub fn issued_value(issued: &Issued) -> Value {
    let fields: BTreeMap<Symbol, Value> = BTreeMap::from([
        (Symbol::new("certificate"), Value::str(&issued.certificate)),
        (Symbol::new("key"), Value::str(&issued.key)),
        (Symbol::new("der"), Value::bytes(&issued.der)),
        (Symbol::new("fingerprint"), Value::str(&issued.fingerprint)),
    ]);
    Value::Record(Arc::new(fields.into_iter().collect()))
}
