//! The registry end to end: the service `crates/ply-registry/ply` run by `ply run --host` on a port
//! of its own, and the commands that talk to it — `ply publish`, `ply resolve`, `ply yank` — over a
//! real socket, with the store a directory the test reads back.

use crate::harness::{
    Reservation, connect_when_ready, json_of, ply, process, repo, stderr_of, stdout_of,
};
use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::time::Duration;

/// The registry is a program that pulls in the compiler's manifest judgment, so its first run
/// compiles more than a hello-world does.
const STARTUP: Duration = Duration::from_secs(180);

const TOKEN: &str = "t0k-for-orders";

/// The service, killed whatever the test does, including panicking out of an assertion.
struct Registry {
    child: Child,
    url: String,
    store: PathBuf,
    /// The certificate a client names in `PLY_TRUST`, when the registry serves over TLS.
    trust: Option<PathBuf>,
}

impl Registry {
    /// Over plain HTTP, or over `--tls` with `(certificate, key)`.
    fn start(dir: &Path, tls: Option<(&Path, &Path)>) -> Registry {
        let mut reserved = Reservation::take();
        let store = dir.join("store");
        std::fs::create_dir_all(&store).expect("the store is made");
        let tokens = dir.join("tokens.conf");
        std::fs::write(&tokens, format!("token.orders={TOKEN}\n")).expect("the tokens are written");
        let mut command = process(dir);
        command
            .arg("run")
            .arg(repo().join("crates/ply-registry/ply"))
            .arg("--host")
            .arg("--fs")
            .arg(format!("store={}", store.display()))
            .arg("--set")
            .arg(format!("port={}", reserved.port()))
            .arg("--config")
            .arg(&tokens);
        if let Some((certificate, key)) = tls {
            command
                .arg("--tls")
                .arg(format!(
                    "registry={},{}",
                    certificate.display(),
                    key.display()
                ))
                .arg("--set")
                .arg("tls=registry");
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("`ply run` starts the registry");
        let port = reserved.port();
        let secure = tls.is_some();
        // Over plain HTTP, the registry's own answer for a package nobody published proves the
        // port is its; over TLS the reservation already made the port this registry's alone.
        let probe = connect_when_ready(&mut reserved, &mut child, STARTUP, |stream| {
            if secure {
                return true;
            }
            let asked = stream.write_all(
                b"GET /nobody/index.json HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            );
            let mut answer = String::new();
            asked.is_ok()
                && stream.read_to_string(&mut answer).is_ok()
                && answer.starts_with("HTTP/1.1 404")
        });
        if let Err(why) = probe {
            let _ = child.kill();
            panic!("{why}");
        }
        Registry {
            child,
            url: if secure {
                format!("https://localhost:{port}")
            } else {
                format!("http://127.0.0.1:{port}")
            },
            store,
            trust: tls.map(|(certificate, _)| certificate.to_path_buf()),
        }
    }

    fn index(&self, name: &str) -> Value {
        let text = std::fs::read_to_string(self.store.join(name).join("index.json"))
            .expect("the registry wrote the index");
        serde_json::from_str(&text).expect("the index is JSON")
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `ply` in `dir`, pointed at the registry and trusting its certificate, with the publisher's token
/// when `token` says so.
fn asking(dir: &Path, registry: &Registry, token: bool, args: &[&str]) -> Output {
    let mut cmd = ply(dir);
    cmd.env("PLY_REGISTRY", &registry.url);
    if let Some(certificate) = &registry.trust {
        cmd.env("PLY_TRUST", certificate);
    }
    if token {
        cmd.env("PLY_REGISTRY_TOKEN", TOKEN);
    }
    cmd.args(args).output().expect("ply ran")
}

fn said(out: &Output) -> String {
    format!("{}{}", stdout_of(out), stderr_of(out))
}

#[track_caller]
fn ok(out: &Output) {
    assert_eq!(out.status.code(), Some(0), "{}", said(out));
}

#[track_caller]
fn refused(out: &Output, code: &str, words: &str) {
    assert_eq!(out.status.code(), Some(2), "{}", said(out));
    let text = said(out);
    assert!(text.contains(code), "{code} is not in:\n{text}");
    assert!(text.contains(words), "`{words}` is not in:\n{text}");
}

fn replaced(path: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).expect("the file reads");
    assert!(
        text.contains(from),
        "`{}` no longer holds `{from}`",
        path.display()
    );
    std::fs::write(path, text.replacen(from, to, 1)).expect("the file is written");
}

/// A program depending on `orders` from the registry, at least 0.1.0.
fn an_app(dir: &Path) {
    let app = dir.join("app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        app.join("ply.pkg"),
        "import std.pkg (Manifest)\nfn package() -> Manifest = {name: \"app\", version: {major: 0, minor: 1, patch: 0}, prefix: None, runtime: {major: 0, minor: 1, patch: 0}, dependencies: [{name: \"orders\", prefix: None, min: {major: 0, minor: 1, patch: 0}, source: Registry}], entry: None}\n",
    )
    .unwrap();
    std::fs::write(
        app.join("main.ply"),
        "import orders.lib\nfn main() -> Int = lib::double(21)\n",
    )
    .unwrap();
}

fn lock(dir: &Path) -> Value {
    let text = std::fs::read_to_string(dir.join("app/ply.lock")).expect("the lock was written");
    serde_json::from_str(&text).expect("the lock is JSON")
}

#[test]
fn a_library_is_published_resolved_built_and_its_tampered_archive_refused() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let dir = tmp.path();
    let registry = Registry::start(dir, None);

    ok(&ply(dir).args(["new", "orders", "--lib"]).output().unwrap());
    let out = asking(dir, &registry, true, &["publish", "orders", "--json"]);
    ok(&out);
    let v = json_of(&out);
    assert_eq!(v["command"], "publish");
    assert_eq!(v["version"], "0.1.0");
    let published = v["digest"].as_str().expect("a digest").to_string();
    assert_eq!(published.len(), "b3:".len() + 64, "{published}");
    assert_eq!(
        registry.index("orders")["versions"][0]["digest"],
        published.as_str()
    );
    assert_eq!(
        std::fs::read_to_string(registry.store.join("orders/0.1.0/package.plyz.b3")).unwrap(),
        format!("{published}\n")
    );

    // A version is published once; a publish needs a token the name maps to.
    refused(
        &asking(dir, &registry, true, &["publish", "orders"]),
        "E0144",
        "409",
    );
    refused(
        &asking(dir, &registry, false, &["publish", "orders"]),
        "E0141",
        "PLY_REGISTRY_TOKEN",
    );

    // Nothing reads the registry but `ply resolve`: before it, the dependency is not there.
    an_app(dir);
    refused(
        &ply(dir).args(["check", "app"]).output().unwrap(),
        "E0135",
        "has not been fetched",
    );
    ok(&asking(dir, &registry, false, &["resolve", "app"]));
    let pinned = lock(dir);
    assert_eq!(pinned["packages"][0]["name"], "orders");
    assert_eq!(pinned["packages"][0]["version"], "0.1.0");
    assert_eq!(pinned["packages"][0]["archive"], published.as_str());
    assert!(dir.join("app/.ply-cache/registry/orders/ply.pkg").exists());
    assert!(
        dir.join("app/.ply-cache/registry/orders@0.1.0.plyz")
            .exists()
    );

    // Every other command reads the cache, the machine's walk included.
    let out = ply(dir)
        .args(["build", "app", "-o", "app.plyx"])
        .output()
        .unwrap();
    ok(&out);
    let out = ply(dir).args(["run", "app.plyx"]).output().unwrap();
    ok(&out);
    assert!(stdout_of(&out).contains("42"), "{}", said(&out));
    ok(&ply(dir).args(["vendor", "app"]).output().unwrap());
    assert!(dir.join("app/vendor/orders/lib.ply").exists());
    std::fs::remove_dir_all(dir.join("app/vendor")).unwrap();

    // A second version, then the first yanked: a lock that pins it keeps it, and a fresh
    // resolution passes it over.
    replaced(
        &dir.join("orders/ply.pkg"),
        "version: { major: 0, minor: 1, patch: 0 }",
        "version: { major: 0, minor: 2, patch: 0 }",
    );
    ok(&asking(dir, &registry, true, &["publish", "orders"]));
    ok(&asking(dir, &registry, true, &["yank", "orders", "0.1.0"]));
    let index = registry.index("orders");
    assert_eq!(index["versions"][0]["yanked"], true);
    assert_eq!(index["versions"][1]["version"], "0.2.0");
    ok(&asking(dir, &registry, false, &["resolve", "app"]));
    assert_eq!(lock(dir)["packages"][0]["version"], "0.1.0");
    std::fs::remove_file(dir.join("app/ply.lock")).unwrap();
    ok(&asking(dir, &registry, false, &["resolve", "app"]));
    let fresh = lock(dir);
    assert_eq!(fresh["packages"][0]["version"], "0.2.0");

    // The registry now serves other bytes for 0.2.0, sealed and all: what the lock pins decides.
    std::fs::create_dir_all(dir.join("evil")).unwrap();
    for file in ["ply.pkg", "lib.ply"] {
        std::fs::copy(dir.join("orders").join(file), dir.join("evil").join(file)).unwrap();
    }
    replaced(&dir.join("evil/lib.ply"), "n * 2", "n * 3");
    ok(&ply(dir)
        .args(["build", "evil", "-o", "evil.plyz"])
        .output()
        .unwrap());
    std::fs::copy(
        dir.join("evil.plyz"),
        registry.store.join("orders/0.2.0/package.plyz"),
    )
    .unwrap();
    std::fs::remove_dir_all(dir.join("app/.ply-cache")).unwrap();
    refused(
        &asking(dir, &registry, false, &["resolve", "app"]),
        "E0142",
        "`ply.lock` pins it",
    );
    assert_eq!(lock(dir), fresh, "a refused resolve wrote the lock");
    assert!(!dir.join("app/.ply-cache/registry/orders").exists());
    // With no lock, the index's own digest is what the archive is held to.
    std::fs::remove_file(dir.join("app/ply.lock")).unwrap();
    refused(
        &asking(dir, &registry, false, &["resolve", "app"]),
        "E0142",
        "index lists",
    );
}

/// A registry under a certificate no public root signed — a company's own CA, here one issued for
/// the test — is reached by naming that certificate in `PLY_TRUST`, and refused by name without it.
#[test]
fn a_registry_under_a_private_certificate_is_reached_through_ply_trust() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let dir = tmp.path();
    let issued =
        ply_host::certgen::issue(&["localhost".to_string()]).expect("a certificate is issued");
    let certificate = dir.join("registry.pem");
    let key = dir.join("registry.key");
    std::fs::write(&certificate, &issued.certificate).expect("the certificate is written");
    std::fs::write(&key, &issued.key).expect("the key is written");
    let registry = Registry::start(dir, Some((&certificate, &key)));

    ok(&ply(dir).args(["new", "orders", "--lib"]).output().unwrap());
    ok(&asking(dir, &registry, true, &["publish", "orders"]));
    an_app(dir);
    ok(&asking(dir, &registry, false, &["resolve", "app"]));
    assert_eq!(lock(dir)["packages"][0]["version"], "0.1.0");

    // The same registry with nothing naming its certificate: the handshake is what fails.
    let out = ply(dir)
        .env("PLY_REGISTRY", &registry.url)
        .args(["resolve", "app"])
        .output()
        .unwrap();
    refused(&out, "E0141", "did not complete a TLS handshake");
    assert!(said(&out).contains("PLY_TRUST"), "{}", said(&out));

    // A file `PLY_TRUST` names that does not load stops the command before it runs.
    let out = ply(dir)
        .env("PLY_TRUST", dir.join("absent.pem"))
        .args(["check", "app"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "{}", said(&out));
    let text = said(&out);
    assert!(text.contains("E0430"), "{text}");
    assert!(text.contains("absent.pem"), "{text}");
}

#[test]
fn a_program_and_a_path_dependency_are_not_published() {
    let tmp = tempfile::tempdir().expect("a temp dir");
    let dir = tmp.path();
    ok(&ply(dir).args(["new", "app"]).output().unwrap());
    refused(
        &ply(dir).args(["publish", "app"]).output().unwrap(),
        "E0145",
        "is a program",
    );
    ok(&ply(dir).args(["new", "lib", "--lib"]).output().unwrap());
    ok(&ply(dir).args(["new", "base", "--lib"]).output().unwrap());
    replaced(
        &dir.join("lib/ply.pkg"),
        "dependencies: [],",
        "dependencies: [{ name: \"base\", prefix: None, min: { major: 0, minor: 1, patch: 0 }, source: Path(\"../base\") }],",
    );
    refused(
        &ply(dir).args(["publish", "lib"]).output().unwrap(),
        "E0145",
        "the path `../base`",
    );
}

/// The service's own tests: the store over a twin, the upload judgment, and every route.
#[test]
fn the_registrys_own_suite_runs_green() {
    let out = ply(&repo())
        .args(["test", "crates/ply-registry/ply", "--no-cache", "--json"])
        .output()
        .unwrap();
    let report = json_of(&out);
    assert_eq!(report["ok"], true, "{}", said(&out));
    assert_eq!(report["summary"]["failed"], 0, "{}", report["summary"]);
    assert!(
        report["summary"]["passed"].as_u64().unwrap_or(0) >= 13,
        "the suite is smaller than it was: {}",
        report["summary"]
    );
}
