mod blocks;
mod c;
mod detached;
mod emitted;
mod entry;
mod frames;
mod heap;
mod hosted;
mod linked;
mod parallel;
mod rt;
mod secret;
mod stack;
mod tasks;

pub(crate) fn ignored_test_command<F: Fn()>(_test: F) -> std::process::Command {
    let path = std::any::type_name::<F>();
    let name = path.split_once("::").map_or(path, |(_binary, name)| name);
    let mut command = std::process::Command::new(std::env::current_exe().expect("the test binary"));
    command.args([name, "--exact", "--ignored", "--nocapture"]);
    command
}
