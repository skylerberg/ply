use crate::units::{self, def};
use ply_compiled::c::Library;
use ply_compiled::c::linked::install;
use ply_compiled::heap::imm_value;

#[test]
fn a_program_compiled_into_the_executable_loads_from_the_program_table_passed_by_its_main_function()
{
    let dir = tempfile::tempdir().expect("a scratch directory");
    let defs = [def("m.seven", 0, "  return ply_imm(7);")];
    let text = units::text(&defs, &units::exports_of(&defs));
    let Some(path) = units::library(dir.path(), &text) else {
        return;
    };
    let opened = Box::leak(Box::new(Library::open(&path).expect("the unit opens")));
    let unit = std::ptr::from_ref(opened.descriptor().expect("the unit lays a descriptor out"));
    assert!(
        Library::linked().is_none(),
        "nothing is linked before a descriptor is installed"
    );
    assert!(
        unsafe { install(std::ptr::null()) }.is_err(),
        "no descriptor is no program"
    );
    unsafe { install(unit) }.expect("a descriptor installs");
    let native = ply_compiled::c::load_linked().expect("the linked unit loads");
    assert_eq!(native.names(), ["m.seven"]);
    let result = units::entered(&native, "m.seven", &[]).expect("it returns");
    assert_eq!(imm_value(result), 7);
    assert!(
        unsafe { install(unit) }.is_err(),
        "a second descriptor is rejected"
    );
}

fn linked_library(
    dir: &std::path::Path,
    defs: &[units::Def],
    slots: &[&str],
    own: &str,
) -> std::path::PathBuf {
    let rows = |pairs: Vec<(String, String)>| {
        let mut out = String::from("{");
        for (name, cell) in pairs {
            out.push_str(&format!(" {{ \"{name}\", (void *){cell} }},"));
        }
        out.push_str(" { 0, 0 } }");
        out
    };
    let declared = (slots.iter())
        .map(|s| format!("Word (*{s}_slot)(PlyCtx *);\n"))
        .collect::<String>();
    let links = format!(
        "{declared}struct ply_name_cell {{ const char *name; void *cell; }};\n\
         const struct ply_name_cell ply_slots[] = {};\n\
         const struct ply_name_cell ply_exported[] = {};\n\
         const struct ply_name_cell ply_keys[] = {{ {{ 0, 0 }} }};\n\
         const struct ply_name_cell ply_field_words[] = {{ {{ 0, 0 }} }};\n\
         const char *const ply_code_names[] = {{ 0 }};\n\
         struct ply_links {{ const struct ply_name_cell *slots, *exported, *keys, *field_words; \
         const char *const *code_names; }};\n\
         const struct ply_links ply_links = {{ ply_slots, ply_exported, ply_keys, ply_field_words, \
         ply_code_names }};\n",
        rows(
            slots
                .iter()
                .map(|s| (s.to_string(), format!("&{s}_slot")))
                .collect()
        ),
        rows(vec![(own.to_string(), format!("&{own}"))]),
    );
    let text = units::text(defs, &units::exports_of(defs))
        .replace(
            "(void *)ply_bind, (void *)0,",
            "(void *)ply_bind, (void *)&ply_links,",
        )
        .replace(
            "const struct ply_descriptor ply_descriptor",
            "extern const struct ply_links ply_links;\nconst struct ply_descriptor ply_descriptor",
        );
    let dir = dir.join(own);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    units::library(&dir, &format!("{text}{links}")).expect("a C compiler")
}

#[test]
fn a_set_of_libraries_fills_the_slots_of_calls_between_them_in_both_directions() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let one = units::symbol("m.one");
    let two = units::symbol("m.two");
    let first = [def(
        "m.one",
        0,
        &format!("  extern Word (*{two}_slot)(PlyCtx *);\n  return {two}_slot(ctx);"),
    )];
    let second = [def(
        "m.two",
        0,
        &format!("  extern Word (*{one}_slot)(PlyCtx *);\n  return ply_imm({one}_slot ? 7 : 0);"),
    )];
    if std::process::Command::new("cc")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let a = linked_library(dir.path(), &first, &[two.as_str()], &one);
    let b = linked_library(dir.path(), &second, &[one.as_str()], &two);
    let native = ply_compiled::c::load_libraries(&[a, b]).expect("the libraries load");
    let result = units::entered(&native, "m.one", &[]).expect("it returns");
    assert_eq!(imm_value(result), 7);
}
