use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Can create a TLS listener, which is what puts `ply_host::tls::listen` into the listing.
const SECURE: &str = "\
import std.net

fn main() -> Int / {net::net.write[listener]} = {
  let l = net::net.listen_tls[listener](0, \"api\");
  net::net.close[listener](l);
  l
}
";

/// A plaintext listener whose row still admits `net.listen_tls`: a row names a resource and a mode, not an operation.
const PLAIN: &str = "\
import std.net

fn main() -> Int / {net::net.write[listener]} = {
  let l = net::net.listen[listener](0);
  net::net.close[listener](l);
  l
}
";

/// Reaches the boundary without touching a socket.
const NO_SOCKET: &str = "\
fn main() -> Int = simulate { spawn { 1 }; 2 }
";

fn project(source: &str) -> TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(dir.path().join("m.ply"), source).expect("the fixture is written");
    dir
}

fn ply(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("ply").expect("the binary is built");
    cmd.arg("--color").arg("never").current_dir(dir);
    cmd
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is utf-8")
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is utf-8")
}

/// Generated rather than checked in: a private key in a repository leaks.
fn credential(dir: &Path, name: &str) -> (PathBuf, PathBuf) {
    let key = rcgen::KeyPair::generate().expect("a key pair");
    let cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .expect("certificate parameters")
        .self_signed(&key)
        .expect("a self-signed certificate");
    let cert_path = dir.join(format!("{name}.pem"));
    let key_path = dir.join(format!("{name}.key"));
    std::fs::write(&cert_path, cert.pem()).expect("the certificate is written");
    std::fs::write(&key_path, key.serialize_pem()).expect("the key is written");
    (cert_path, key_path)
}

fn spec(name: &str, cert: &Path, key: &Path) -> String {
    format!("{name}={},{}", cert.display(), key.display())
}

#[test]
fn the_listing_discloses_the_tls_stack_and_the_credential_it_was_given() {
    let dir = project(SECURE);
    let (cert, key) = credential(dir.path(), "api");
    let output = ply(dir.path())
        .arg("hosts")
        .arg("--host")
        .arg("--tls")
        .arg(spec("api", &cert, &key))
        .output()
        .expect("ply ran");
    assert!(output.status.success(), "{}", stderr_of(&output));
    let text = stdout_of(&output);

    assert!(
        text.contains("ply_host::tls::listen"),
        "`net.listen_tls` must have its own line with its own handler:\n{text}"
    );
    assert!(text.contains("   transport"), "{text}");
    assert!(
        text.contains("tls  rustls 0.23.43 · provider ring · TLS 1.3, TLS 1.2 · alpn http/1.1"),
        "{text}"
    );
    assert!(text.contains("   credentials"), "{text}");
    assert!(
        text.contains("api  sha256:") && text.contains("1 certificate"),
        "the credential must be named with its fingerprint:\n{text}"
    );
    assert!(text.contains("digest: b3:"), "{text}");
}

#[test]
fn a_program_that_touches_no_socket_reports_no_transport() {
    let dir = project(NO_SOCKET);
    let text = stdout_of(&ply(dir.path()).arg("hosts").arg("--host").output().unwrap());
    assert!(!text.contains("transport"), "{text}");
    assert!(!text.contains("credentials"), "{text}");
    assert!(!text.contains("ply_host::tls"), "{text}");
}

#[test]
fn a_plaintext_program_whose_row_admits_listen_tls_still_discloses_it() {
    let dir = project(PLAIN);
    let text = stdout_of(&ply(dir.path()).arg("hosts").arg("--host").output().unwrap());
    assert!(text.contains("ply_host::tcp::listen"), "{text}");
    assert!(text.contains("ply_host::tls::listen"), "{text}");
    assert!(text.contains("   transport"), "{text}");
}

#[test]
fn a_tls_program_with_no_credential_is_told_what_will_happen() {
    let dir = project(SECURE);
    let text = stdout_of(&ply(dir.path()).arg("hosts").arg("--host").output().unwrap());
    assert!(text.contains("   transport"), "{text}");
    assert!(
        text.contains("`net.listen_tls` is E0429 until `--tls NAME=CERT,KEY` names one"),
        "{text}"
    );
}

#[test]
fn the_digest_is_stable_across_a_rotation_and_moves_when_a_credential_is_added() {
    let dir = project(SECURE);
    let (cert, key) = credential(dir.path(), "api");

    let digest = |args: Vec<String>| {
        let mut cmd = ply(dir.path());
        cmd.arg("hosts").arg("--host").arg("--digest");
        for arg in args {
            cmd.arg(arg);
        }
        stdout_of(&cmd.output().expect("ply ran"))
            .trim()
            .to_string()
    };

    let one = vec!["--tls".to_string(), spec("api", &cert, &key)];
    let before = digest(one.clone());
    assert!(before.starts_with("b3:"), "{before}");

    // The same credential name, a different certificate: an operational fact.
    let (rotated, rotated_key) = credential(dir.path(), "rotated");
    assert_eq!(
        before,
        digest(vec![
            "--tls".to_string(),
            spec("api", &rotated, &rotated_key)
        ]),
        "a renewed certificate must not move the digest"
    );

    // A second credential: a structural change to what the run can serve.
    let (admin, admin_key) = credential(dir.path(), "admin");
    let mut two = one.clone();
    two.push("--tls".to_string());
    two.push(spec("admin", &admin, &admin_key));
    assert_ne!(before, digest(two), "a second credential must move it");

    assert_ne!(before, digest(Vec::new()), "removing one must move it");
}

#[test]
fn the_json_object_carries_the_transport_and_the_full_fingerprint() {
    let dir = project(SECURE);
    let (cert, key) = credential(dir.path(), "api");
    let output = ply(dir.path())
        .arg("hosts")
        .arg("--host")
        .arg("--json")
        .arg("--tls")
        .arg(spec("api", &cert, &key))
        .output()
        .unwrap();
    let text = stdout_of(&output);
    let report: Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("stdout was not one object: {e}\n{text}"));

    assert_eq!(report["command"], "hosts");
    assert_eq!(report["transport"]["library"], "rustls");
    assert_eq!(report["transport"]["provider"], "ring");
    assert_eq!(report["transport"]["alpn"], serde_json::json!(["http/1.1"]));
    let credential = &report["transport"]["credentials"][0];
    assert_eq!(credential["name"], "api");
    assert_eq!(credential["certificates"], 1);
    let fingerprint = credential["fingerprint"].as_str().expect("a fingerprint");
    assert!(fingerprint.starts_with("sha256:"), "{fingerprint}");
    assert_eq!(
        fingerprint.len(),
        "sha256:".len() + 64,
        "the object must carry the whole digest, not the abbreviation"
    );
}

#[test]
fn a_credential_that_does_not_load_is_e0430_naming_the_file() {
    let dir = project(SECURE);
    let (cert, key) = credential(dir.path(), "api");
    let broken = dir.path().join("broken.pem");
    std::fs::write(&broken, "-----BEGIN CERTIFICATE-----\nnope\n").unwrap();
    let missing = dir.path().join("absent.pem");

    for bad in [spec("api", &broken, &key), spec("api", &missing, &key)] {
        let output = ply(dir.path())
            .arg("run")
            .arg("--host")
            .arg("--tls")
            .arg(&bad)
            .output()
            .expect("ply ran");
        let rendered = format!("{}{}", stdout_of(&output), stderr_of(&output));
        assert_eq!(output.status.code(), Some(2), "{rendered}");
        assert!(rendered.contains("E0430"), "{rendered}");
    }

    // A key that is not the leaf's needs both files to catch, and is the one a deploy gets wrong.
    let (_, other_key) = credential(dir.path(), "other");
    let output = ply(dir.path())
        .arg("run")
        .arg("--host")
        .arg("--tls")
        .arg(spec("api", &cert, &other_key))
        .output()
        .unwrap();
    let rendered = format!("{}{}", stdout_of(&output), stderr_of(&output));
    assert!(rendered.contains("E0430"), "{rendered}");
}

/// A mistyped argument needs the form; a broken PEM needs the file.
#[test]
fn a_malformed_argument_is_refused_with_the_form_rather_than_a_diagnostic() {
    let dir = project(SECURE);
    let output = ply(dir.path())
        .arg("run")
        .arg("--host")
        .arg("--tls")
        .arg("api")
        .output()
        .unwrap();
    let rendered = stderr_of(&output);
    assert!(!output.status.success());
    assert!(
        rendered.contains("--tls NAME=CERT.pem,KEY.pem"),
        "{rendered}"
    );
    assert!(!rendered.contains("E0430"), "{rendered}");
}

#[test]
fn tls_without_host_is_refused() {
    let dir = project(SECURE);
    for command in ["run", "test", "hosts"] {
        let output = ply(dir.path())
            .arg(command)
            .arg("--tls")
            .arg("api=a.pem,b.key")
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "`ply {command} --tls` without `--host` must be refused"
        );
    }
}

#[test]
fn a_hermetic_run_reaches_no_credential_and_says_so() {
    let dir = project(SECURE);
    let output = ply(dir.path()).arg("run").output().expect("ply ran");
    let rendered = format!("{}{}", stdout_of(&output), stderr_of(&output));
    assert!(rendered.contains("E0424"), "{rendered}");
    assert!(
        rendered.contains("ply_host::tls::listen"),
        "E0424 must name the handler that would have served it:\n{rendered}"
    );
}
