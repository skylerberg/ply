use ply_corpus::model::*;

#[test]
fn an_atom_renders_as_the_row_syntax_it_will_be_parsed_from() {
    assert_eq!(
        Atom::read(Eff::Db, "users").render(),
        "effects::db.read[users]"
    );
    assert_eq!(
        Atom::write(Eff::Cache, "hot").render(),
        "effects::cache.write[hot]"
    );
    assert_eq!(
        Atom::singleton_read(Eff::Clock).render(),
        "effects::clock.read"
    );
}

#[test]
fn clamp_agrees_with_the_prelude_definition_it_mirrors() {
    assert_eq!(clamp(100_004), 1);
    assert_eq!(clamp(-1), -1);
    assert_eq!(mix(2, 3), 2 * 31 + 3 * 17 + 7);
}

#[test]
fn a_module_binder_is_its_last_dotted_segment() {
    let module = Module {
        id: 0,
        name: "store0.rules_3".into(),
        path: "store0/rules_3.ply".into(),
        layer: 0,
        imports: Vec::new(),
        defs: Vec::new(),
        helper: Helper {
            name: "stage_0".into(),
            m: 3,
            b: 1,
        },
        status_type: "Status0".into(),
        ctor_ready: "Ready0".into(),
        ctor_idle: "Idle0".into(),
        needs_effects: false,
    };
    assert_eq!(module.binder(), "rules_3");
}

#[test]
fn a_written_table_is_the_only_one_a_footprint_reports_writing() {
    let tables = vec!["users".to_string(), "orders".to_string()];
    let regions = vec!["hot".to_string()];
    let append = Shape::TableAppend {
        table: 1,
        a: 2,
        b: 3,
    };
    let atoms = append.own_atoms(&tables, &regions);
    assert_eq!(atoms.len(), 2);
    assert!(atoms.contains(&Atom::write(Eff::Db, "orders")));
    assert!(!atoms.contains(&Atom::write(Eff::Db, "users")));
}
