use ply_core::{Effect, Mode, Resource, Symbol, label_var_name};

#[test]
fn an_effect_with_an_operation_name_prints_the_operation_name_instead_of_read_or_write() {
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
fn resource_variables_are_named_l_m_n_then_l1_and_onward_as_their_index_grows() {
    assert_eq!(label_var_name(0), "l");
    assert_eq!(label_var_name(2), "n");
    assert_eq!(label_var_name(3), "l1");
    assert_eq!(
        Effect::operation("net", Resource::Var(1), Mode::Write, "recv").to_string(),
        "net.recv[m]"
    );
}
