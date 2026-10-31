use ply_core::{Effect, Mode, Resource, Symbol, label_var_name};

#[test]
fn an_operation_atom_prints_its_operation_in_place_of_the_mode() {
    let conn = || Resource::Named(Symbol::new("conn"));
    assert_eq!(
        Effect::operation("net", conn(), Mode::Write, "send").to_string(),
        "net.send[conn]"
    );
    assert_eq!(
        Effect::new("net", conn(), Mode::Write).to_string(),
        "net.write[conn]"
    );
    assert_eq!(
        Effect::new("clock", Resource::Singleton, Mode::Read).to_string(),
        "clock.read"
    );
}

#[test]
fn a_label_variable_is_named_by_its_place() {
    assert_eq!(label_var_name(0), "l");
    assert_eq!(label_var_name(2), "n");
    assert_eq!(label_var_name(3), "l1");
    assert_eq!(
        Effect::operation("net", Resource::Var(1), Mode::Write, "recv").to_string(),
        "net.recv[m]"
    );
}
