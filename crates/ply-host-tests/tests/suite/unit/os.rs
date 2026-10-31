use ply_host::os::*;

fn said(program: &str, args: &[&str]) -> String {
    let done = std::process::Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("`{program}` runs: {e}"));
    String::from_utf8_lossy(&done.stdout).trim().to_string()
}

#[cfg(unix)]
#[test]
fn the_process_and_machine_information_matches_what_the_operating_system_commands_report() {
    assert_eq!(name().expect("uname returns"), said("uname", &["-sr"]));
    assert_eq!(
        hostname().expect("the kernel names the machine"),
        said("uname", &["-n"])
    );
    assert_eq!(uid().to_string(), said("id", &["-u"]));
    if let Some(user) = user_name(uid()) {
        assert_eq!(user, said("id", &["-un"]));
    }
    assert_eq!(
        executable().expect("a test binary has a path"),
        std::env::current_exe()
            .and_then(|path| path.canonicalize())
            .expect("a test binary has a path")
            .to_string_lossy()
    );
}

#[test]
fn a_user_id_that_does_not_exist_has_no_user_name() {
    assert_eq!(user_name(-1), None);
}

#[test]
fn the_processors_are_the_ones_the_process_may_use() {
    let cpus = cpus();
    assert!(cpus >= 1);
    assert_eq!(
        cpus,
        std::thread::available_parallelism().map_or(1, |n| n.get() as i64)
    );
}
