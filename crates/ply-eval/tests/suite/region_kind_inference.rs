//! What [`ply_eval::region_kind`] decides, on programs written in Ply.

use ply_eval::RegionKind;
use ply_eval::region_kind::{Cause, Region, Regions, check, infer};
use ply_span::{Diagnostic, SourceId, SourceMap, Span};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};

fn load(src: &str) -> (Program, Resolved) {
    let mut map = SourceMap::new();
    let id: SourceId = map.add("kinds.ply", src.to_string());
    let mut program = match parse_program([(id, ModuleName::from_dotted("kinds"), src)]) {
        Ok(p) => p,
        Err(ds) => panic!("the probe must parse: {ds:#?}\n{src}"),
    };
    let resolved = resolve(&mut program).expect("the probe must resolve");
    (program, resolved)
}

#[track_caller]
fn regions_of(src: &str) -> Regions {
    let (program, resolved) = load(src);
    let regions = infer(&program, &resolved);
    assert!(
        !regions.is_empty(),
        "this probe opens no region, so it decides nothing\n{src}"
    );
    regions
}

/// The region carrying this brand.
#[track_caller]
fn region<'a>(regions: &'a Regions, brand: &str) -> &'a Region {
    regions
        .iter()
        .find(|r| r.brand.as_str() == brand)
        .unwrap_or_else(|| {
            panic!(
                "no region branded `{brand}`; found {:?}",
                regions.iter().map(|r| r.brand.as_str()).collect::<Vec<_>>()
            )
        })
}

#[track_caller]
fn kind_of(src: &str, brand: &str) -> RegionKind {
    region(&regions_of(src), brand).kind
}

const AMB: &str = r#"
effect amb { read flip[coin]() -> Bool }
"#;

/// The case the region-kind rule says is common and free: a region that allocates, reads and writes its own
/// cells and performs nothing.
#[test]
fn a_region_that_performs_nothing_and_handles_nothing_is_unique() {
    let src = r#"
fn double(n: Int) -> Int = n * 2

fn total() -> Int =
  with_cell[acc](0) { c -> { cell_set(c, double(21)); cell_get(c) } }
"#;
    assert_eq!(kind_of(src, "acc"), RegionKind::Unique);
    assert!(region(&regions_of(src), "acc").capture.is_none());
}

/// Nesting does not make a region shared.
#[test]
fn nested_pure_regions_are_both_unique() {
    let src = r#"
fn nested() -> Int =
  with_cell[outer](1) { a ->
    with_cell[inner](2) { b -> cell_get(a) + cell_get(b) } }
"#;
    let regions = regions_of(src);
    assert_eq!(regions.len(), 2);
    assert_eq!(region(&regions, "outer").kind, RegionKind::Unique);
    assert_eq!(region(&regions, "inner").kind, RegionKind::Unique);
    assert_eq!(regions.unique(), 2);
}

/// A `with_cell[r]` written inside `with_region[r]` allocates into that region rather than opening
/// one of its own.
#[test]
fn a_cell_inside_a_region_of_its_own_brand_opens_no_second_region() {
    let src = r#"
fn shaped() -> Int =
  with_region[r] { with_cell[r](7) { c -> cell_get(c) } }
"#;
    let regions = regions_of(src);
    assert_eq!(regions.len(), 1, "{:?}", regions);
    assert_eq!(region(&regions, "r").kind, RegionKind::Unique);
}

#[test]
fn a_cell_of_a_different_brand_inside_a_region_opens_its_own() {
    let src = r#"
fn shaped() -> Int =
  with_region[r] { with_cell[s](7) { c -> cell_get(c) } }
"#;
    assert_eq!(regions_of(src).len(), 2);
}

/// A higher-order program with no handler, no `simulate` and no `task` has no capture for an
/// unknown callee to reach, so the unknown callee costs nothing.
#[test]
fn an_unknown_callee_in_a_program_with_no_capture_stays_unique() {
    let src = r#"
fn apply(f: (Int) -> Int, n: Int) -> Int = f(n)

fn go() -> Int =
  with_cell[acc](0) { c -> { cell_set(c, apply(|x| x + 1, 1)); cell_get(c) } }
"#;
    assert_eq!(kind_of(src, "acc"), RegionKind::Unique);
}

/// Through a handler written inside the region.
#[test]
fn a_general_clause_inside_the_region_makes_it_shared() {
    let src = &format!(
        r#"{AMB}
fn search() -> Int =
  with_cell[trace](0) {{ c ->
    handle {{ if amb.flip[coin]() {{ cell_get(c) }} else {{ 0 }} }} with {{
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    }} }}
"#
    );
    let regions = regions_of(src);
    let r = region(&regions, "trace");
    assert_eq!(r.kind, RegionKind::Shared);
    let site = r.capture.as_ref().expect("a shared region names its site");
    assert!(
        matches!(&site.cause, Cause::Clause { effect, op }
            if effect.as_str() == "kinds.amb" && op.as_str() == "flip"),
        "{:?}",
        site.cause
    );
    assert!(site.through.is_empty(), "the site is written in the region");
}

/// A tail-resumptive clause captures, and its continuation still cannot outlive
/// the region.
#[test]
fn a_tail_resumptive_clause_inside_the_region_leaves_it_unique() {
    let src = &format!(
        r#"{AMB}
fn once() -> Int =
  with_cell[trace](0) {{ c ->
    handle {{ if amb.flip[coin]() {{ 1 }} else {{ 0 }} }} with {{
      amb.flip[coin]() -> {{ cell_set(c, 1); true }},
      return x -> x
    }} }}
"#
    );
    let regions = regions_of(src);
    let r = region(&regions, "trace");
    assert_eq!(r.kind, RegionKind::Unique);
    assert!(
        r.capture.is_none(),
        "a tail-resumptive clause is not a capture that outlives the region: {:?}",
        r.capture
    );
}

/// The handler is the caller's, so the capture crosses the region's boundary and this analysis
/// cannot see the other side of it.
#[test]
fn a_perform_the_region_does_not_answer_makes_it_shared() {
    let src = &format!(
        r#"{AMB}
fn inside() -> Bool =
  with_cell[trace](0) {{ c -> {{ cell_set(c, 1); amb.flip[coin]() }} }}
"#
    );
    let regions = regions_of(src);
    let r = region(&regions, "trace");
    assert_eq!(r.kind, RegionKind::Shared);
    assert!(
        matches!(&r.capture.as_ref().expect("a site").cause, Cause::Escapes { effect, .. }
            if effect.as_str() == "kinds.amb"),
        "{:?}",
        r.capture
    );
}

/// Through a called function: the capture is written two definitions away and the diagnostic has to
/// be able to say so.
#[test]
fn a_capture_reachable_through_a_called_function_makes_the_region_shared() {
    let src = &format!(
        r#"{AMB}
fn deepest() -> Int =
  handle {{ if amb.flip[coin]() {{ 1 }} else {{ 2 }} }} with {{
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }}

fn middle() -> Int = deepest()

fn outer() -> Int =
  with_cell[acc](0) {{ c -> {{ cell_set(c, middle()); cell_get(c) }} }}
"#
    );
    let regions = regions_of(src);
    let r = region(&regions, "acc");
    assert_eq!(r.kind, RegionKind::Shared);
    let site = r.capture.as_ref().expect("a shared region names its site");
    let chain: Vec<&str> = site.through.iter().map(|s| s.as_str()).collect();
    assert_eq!(chain, ["kinds.middle", "kinds.deepest"]);
    assert_eq!(
        site.chain().as_deref(),
        Some("reached through `kinds.middle` → `kinds.deepest`")
    );
}

/// Through a task: the scheduler parks the performing task and resumes it, which is a capture
/// whoever wrote it.
#[test]
fn a_task_spawned_in_the_region_makes_it_shared() {
    let src = r#"
fn work() -> Unit = ()

fn spawner() -> Unit =
  with_cell[acc](0) { c -> { let t = task.spawn(|| work()); task.join(t) } }
"#;
    let regions = regions_of(src);
    let r = region(&regions, "acc");
    assert_eq!(r.kind, RegionKind::Shared);
    assert!(
        matches!(&r.capture.as_ref().expect("a site").cause, Cause::Task { op }
            if op.as_str() == "spawn"),
        "{:?}",
        r.capture
    );
}

/// Through a task reached from a called function, which is the shape a service has: the accept loop
/// spawns, and the region is opened by whatever called it.
#[test]
fn a_task_spawned_by_a_called_function_makes_the_region_shared() {
    let src = r#"
fn work() -> Unit = ()

fn serve() -> Unit = { let t = task.spawn(|| work()); task.join(t) }

fn run() -> Unit = with_cell[acc](0) { c -> serve() }
"#;
    let regions = regions_of(src);
    let r = region(&regions, "acc");
    assert_eq!(r.kind, RegionKind::Shared);
    let site = r.capture.as_ref().expect("a site");
    assert_eq!(
        site.through.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        ["kinds.serve"]
    );
}

#[test]
fn a_simulate_in_the_region_makes_it_shared() {
    let src = r#"
fn go() -> Int =
  with_cell[acc](0) { c -> simulate { cell_get(c) } }
"#;
    let regions = regions_of(src);
    assert_eq!(region(&regions, "acc").kind, RegionKind::Shared);
    assert!(matches!(
        region(&regions, "acc")
            .capture
            .as_ref()
            .expect("a site")
            .cause,
        Cause::Simulate
    ));
}

/// A callee held in a local binding could be any closure in the program, and this program has a
/// capture for it to be.
#[test]
fn an_unknown_callee_in_a_program_that_captures_makes_the_region_shared() {
    let src = &format!(
        r#"{AMB}
fn backtrack() -> Int =
  handle {{ if amb.flip[coin]() {{ 1 }} else {{ 2 }} }} with {{
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }}

fn apply(f: () -> Int) -> Int =
  with_cell[acc](0) {{ c -> {{ cell_set(c, f()); cell_get(c) }} }}
"#
    );
    let regions = regions_of(src);
    let r = region(&regions, "acc");
    assert_eq!(r.kind, RegionKind::Shared);
    assert!(
        matches!(&r.capture.as_ref().expect("a site").cause, Cause::Indirect),
        "{:?}",
        r.capture
    );
}

/// The region-kind rule's own two-resumption example with `handle` and `with_cell` swapped, which is the
/// shape every backtracking handler over scratch state has.
#[test]
fn a_handle_enclosing_the_region_does_not_hide_the_capture() {
    let src = &format!(
        r#"{AMB}
fn search() -> Int =
  handle {{
    with_cell[trace](0) {{ c -> {{
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b {{ cell_get(c) }} else {{ cell_get(c) * 10 }}
    }} }}
  }} with {{
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }}
"#
    );
    let regions = regions_of(src);
    let r = region(&regions, "trace");
    assert_eq!(r.kind, RegionKind::Shared);
    assert!(
        matches!(&r.capture.as_ref().expect("a site").cause, Cause::Escapes { effect, op }
            if effect.as_str() == "kinds.amb" && op.as_str() == "flip"),
        "the perform is answered outside the region, so it escapes it: {:?}",
        r.capture
    );
}

/// The same, with the enclosing clause **tail-resumptive**.
#[test]
fn a_tail_resumptive_handle_enclosing_the_region_does_not_hide_the_capture() {
    let src = &format!(
        r#"{AMB}
fn once() -> Int =
  handle {{
    with_cell[trace](0) {{ c -> {{ cell_set(c, 1); if amb.flip[coin]() {{ 1 }} else {{ 0 }} }} }}
  }} with {{
    amb.flip[coin]() -> true,
    return x -> x
  }}
"#
    );
    assert_eq!(kind_of(src, "trace"), RegionKind::Shared);
}

/// Every shape of region, under one enclosing handler: `with_region[r]` with a `with_cell[r]`
/// inside it — the region syntax — two nested regions, and a region opened inside a `map`
/// callback.
#[test]
fn the_enclosing_handle_hides_the_capture_for_no_shape_of_region() {
    let shapes: [(&str, &[&str]); 4] = [
        (
            "with_region[r] { with_cell[r](0) { c -> if amb.flip[coin]() { cell_get(c) } else { 0 } } }",
            &["r"],
        ),
        (
            "with_region[r] { with_cell[s](0) { c -> if amb.flip[coin]() { cell_get(c) } else { 0 } } }",
            &["r", "s"],
        ),
        (
            "with_cell[outer](0) { a -> with_cell[inner](0) { b -> if amb.flip[coin]() { cell_get(a) } else { cell_get(b) } } }",
            &["outer", "inner"],
        ),
        (
            "with_cell[trace](0) { c -> fold(map([1, 2], |x| if amb.flip[coin]() { x } else { cell_get(c) }), 0, |a, b| a + b) }",
            &["trace"],
        ),
    ];
    for (body, brands) in shapes {
        let src = format!(
            r#"{AMB}
fn shaped() -> Int =
  handle {{ {body} }} with {{
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }}
"#
        );
        let regions = regions_of(&src);
        for brand in brands {
            assert_eq!(
                region(&regions, brand).kind,
                RegionKind::Shared,
                "`{brand}` in `{body}`"
            );
        }
    }
}

/// The analysis must not depend on where the `perform` is *written*.
#[test]
fn hoisting_the_perform_into_a_helper_does_not_move_the_inferred_kind() {
    let inline = &format!(
        r#"{AMB}
fn search() -> Int =
  handle {{ with_cell[trace](0) {{ c -> if amb.flip[coin]() {{ cell_get(c) }} else {{ 0 }} }} }}
  with {{ amb.flip[coin]() resume k -> k(true) + k(false), return x -> x }}
"#
    );
    let hoisted = &format!(
        r#"{AMB}
fn coin() -> Bool = amb.flip[coin]()

fn search() -> Int =
  handle {{ with_cell[trace](0) {{ c -> if coin() {{ cell_get(c) }} else {{ 0 }} }} }}
  with {{ amb.flip[coin]() resume k -> k(true) + k(false), return x -> x }}
"#
    );
    assert_eq!(kind_of(inline, "trace"), RegionKind::Shared);
    assert_eq!(kind_of(hoisted, "trace"), RegionKind::Shared);
}

/// The annotation is the backstop, so it has to fire on the same programs the inference does — the region-kind rule:
/// forcing `unique` where a capture is reachable "is a compile error naming the capture
/// site".
#[test]
fn forcing_unique_over_a_capture_an_enclosing_handle_answers_is_refused() {
    let src = &format!(
        r#"{AMB}
fn search() -> Int =
  handle {{
    with_cell[trace](0) {{ c -> {{
      let b = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      if b {{ cell_get(c) }} else {{ cell_get(c) * 10 }}
    }} }}
  }} with {{
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }}
"#
    );
    let ds = refusals(src, "trace", RegionKind::Unique);
    assert_eq!(ds.len(), 1, "{ds:#?}");
    assert_eq!(ds[0].code, ply_span::codes::REGION_KIND_REFUSED);
    assert!(ds[0].message.contains("`trace`"), "{}", ds[0].message);
}

/// An outer region is shared whenever an inner one is: the inner region's body is part of the outer
/// region's body.
#[test]
fn an_inner_regions_capture_makes_the_enclosing_region_shared_too() {
    let src = &format!(
        r#"{AMB}
fn nested() -> Bool =
  with_cell[outer](0) {{ a ->
    with_cell[inner](0) {{ b -> amb.flip[coin]() }} }}
"#
    );
    let regions = regions_of(src);
    assert_eq!(region(&regions, "outer").kind, RegionKind::Shared);
    assert_eq!(region(&regions, "inner").kind, RegionKind::Shared);
    assert_eq!(regions.shared(), 2);
}

/// A region the analysis never saw is `shared`, because the safe answer to "was a capture
/// reachable" is always yes.
#[test]
fn a_region_the_inference_never_saw_is_shared() {
    let src = r#"
fn pure() -> Int = with_cell[acc](0) { c -> cell_get(c) }
"#;
    let regions = regions_of(src);
    assert_eq!(regions.kind(Span::DUMMY), RegionKind::Shared);
}

#[track_caller]
fn refusals(src: &str, brand: &str, kind: RegionKind) -> Vec<Diagnostic> {
    let (program, resolved) = load(src);
    let inferred = infer(&program, &resolved);
    let span = region(&inferred, brand).span;
    match check(&program, &resolved, &[(span, kind)]) {
        Ok(_) => Vec::new(),
        Err(ds) => ds,
    }
}

/// The region-kind rule: forcing `unique` where a capture is reachable is a compile error naming the capture
/// site.
#[test]
fn forcing_unique_where_a_capture_is_reachable_is_refused_and_names_the_site() {
    let src = &format!(
        r#"{AMB}
fn search() -> Int =
  with_cell[trace](0) {{ c ->
    handle {{ if amb.flip[coin]() {{ cell_get(c) }} else {{ 0 }} }} with {{
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    }} }}
"#
    );
    let ds = refusals(src, "trace", RegionKind::Unique);
    assert_eq!(ds.len(), 1, "{ds:#?}");
    let d = &ds[0];
    assert_eq!(d.code, ply_span::codes::REGION_KIND_REFUSED);
    assert!(d.message.contains("`trace`"), "{}", d.message);
    assert!(d.message.contains("unique"), "{}", d.message);

    let regions = regions_of(src);
    let site = region(&regions, "trace")
        .capture
        .as_ref()
        .expect("a site")
        .span;
    assert!(
        d.labels.iter().any(|l| !l.primary && l.span == site),
        "the refusal must point at the capture site: {:#?}",
        d.labels
    );
    assert!(
        d.labels.iter().any(|l| l.primary),
        "and at the region: {:#?}",
        d.labels
    );
    assert!(
        d.notes.iter().any(|n| n.contains("resumption")),
        "the notes must say what a wrong `unique` costs: {:#?}",
        d.notes
    );
}

/// The refusal names the *chain*, so a capture written three definitions away is still actionable.
#[test]
fn a_refusal_names_the_definitions_between_the_region_and_the_capture() {
    let src = &format!(
        r#"{AMB}
fn deepest() -> Int =
  handle {{ if amb.flip[coin]() {{ 1 }} else {{ 2 }} }} with {{
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }}

fn middle() -> Int = deepest()

fn outer() -> Int =
  with_cell[acc](0) {{ c -> {{ cell_set(c, middle()); cell_get(c) }} }}
"#
    );
    let ds = refusals(src, "acc", RegionKind::Unique);
    assert_eq!(ds.len(), 1, "{ds:#?}");
    assert!(
        ds[0]
            .notes
            .iter()
            .any(|n| n.contains("`kinds.middle`") && n.contains("`kinds.deepest`")),
        "{:#?}",
        ds[0].notes
    );
}

/// A declaration that agrees with the inference is not a refusal, and neither is one that asks for
/// the conservative kind over no capture at all: declaring `shared` can only cost a copy.
#[test]
fn declaring_the_kind_the_inference_would_have_chosen_is_accepted() {
    let src = r#"
fn pure() -> Int = with_cell[acc](0) { c -> cell_get(c) }
"#;
    assert!(refusals(src, "acc", RegionKind::Unique).is_empty());
    assert!(refusals(src, "acc", RegionKind::Shared).is_empty());

    let (program, resolved) = load(src);
    let span = region(&infer(&program, &resolved), "acc").span;
    let regions = check(&program, &resolved, &[(span, RegionKind::Shared)])
        .expect("declaring `shared` is always accepted");
    let r = region(&regions, "acc");
    assert_eq!(r.kind, RegionKind::Shared);
    assert!(r.declared);
    assert!(
        r.capture.is_none(),
        "a region declared `shared` over no capture has no site to name"
    );
}

/// Two regions declared wrong are two refusals, not one: a run that reported the first and stopped
/// would make fixing a program iterative.
#[test]
fn every_wrongly_declared_region_is_refused() {
    let src = &format!(
        r#"{AMB}
fn a() -> Bool = with_cell[one](0) {{ c -> amb.flip[coin]() }}
fn b() -> Bool = with_cell[two](0) {{ c -> amb.flip[coin]() }}
"#
    );
    let (program, resolved) = load(src);
    let inferred = infer(&program, &resolved);
    let declared: Vec<(Span, RegionKind)> = inferred
        .iter()
        .map(|r| (r.span, RegionKind::Unique))
        .collect();
    let Err(ds) = check(&program, &resolved, &declared) else {
        panic!("both regions reach a capture and both were declared `unique`");
    };
    assert_eq!(ds.len(), 2, "{ds:#?}");
}

/// The same program inferred twice gives the same answer in the same order.
#[test]
fn inference_is_a_function_of_the_program_alone() {
    let src = &format!(
        r#"{AMB}
fn deepest() -> Int =
  handle {{ if amb.flip[coin]() {{ 1 }} else {{ 2 }} }} with {{
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }}
fn mid() -> Int = deepest()
fn one() -> Int = with_cell[a](0) {{ c -> mid() }}
fn two() -> Int = with_cell[b](0) {{ c -> cell_get(c) }}
fn three() -> Int = with_cell[d](0) {{ c -> one() }}
"#
    );
    let first: Vec<(String, RegionKind, Vec<String>)> = regions_of(src)
        .iter()
        .map(|r| {
            (
                r.brand.to_string(),
                r.kind,
                r.capture
                    .as_ref()
                    .map(|c| c.through.iter().map(|s| s.to_string()).collect())
                    .unwrap_or_default(),
            )
        })
        .collect();
    for _ in 0..8 {
        let again: Vec<(String, RegionKind, Vec<String>)> = regions_of(src)
            .iter()
            .map(|r| {
                (
                    r.brand.to_string(),
                    r.kind,
                    r.capture
                        .as_ref()
                        .map(|c| c.through.iter().map(|s| s.to_string()).collect())
                        .unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(first, again);
    }
    assert_eq!(first.len(), 3);
}

/// What the rule decides on the repository's own programs.
#[test]
fn the_split_over_the_repositorys_own_examples() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels under the workspace root")
        .join("examples");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&root)
        .expect("the repository ships examples")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ply"))
        .collect();
    files.sort();

    let mut map = SourceMap::new();
    let mut loaded: Vec<(SourceId, ModuleName, String)> = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).expect("an example is readable");
        let relative = path.strip_prefix(&root).unwrap_or(path);
        let name = ModuleName::from_relative_path(relative).expect("an example names a module");
        let id = map.add(path, text.clone());
        loaded.push((id, name, text));
    }
    // Demand-driven, exactly as `ply`'s own loader is.
    let mut next = 0;
    while next < loaded.len() {
        let (id, name, text) = loaded[next].clone();
        next += 1;
        let Ok(module) = ply_syntax::parse_module(id, name, &text) else {
            continue;
        };
        for wanted in module.imports.iter().map(|i| i.module_name()) {
            if !ply_std::is_std(&wanted) || loaded.iter().any(|(_, n, _)| *n == wanted) {
                continue;
            }
            let Some(source) = ply_std::source(&wanted) else {
                continue;
            };
            let id = map.add(ply_std::pseudo_path(&wanted), source.to_string());
            loaded.push((id, wanted, source.to_string()));
        }
    }
    let inputs: Vec<_> = loaded
        .iter()
        .map(|(id, name, text)| (*id, name.clone(), text.as_str()))
        .collect();
    let mut program = parse_program(inputs).expect("the examples parse");
    assert!(
        ply_derive::expand_program(&mut program).is_empty(),
        "the examples expand"
    );
    let resolved = resolve(&mut program).expect("the examples resolve");

    let regions = infer(&program, &resolved);
    println!(
        "\n  examples/ and the std modules they import: {} regions, {} unique, {} shared",
        regions.len(),
        regions.unique(),
        regions.shared()
    );
    // The *first* cause found, which is what the diagnostic would name.
    let mut tally: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for r in regions.iter() {
        let cause = match r.capture.as_ref().map(|c| &c.cause) {
            None => "no capture reachable — unique",
            Some(Cause::Clause { .. }) => "a clause binding `resume`",
            Some(Cause::TailClause { .. }) => "a tail-resumptive clause",
            Some(Cause::Escapes { .. }) => "a perform answered outside the region",
            Some(Cause::Task { .. }) => "a task operation",
            Some(Cause::Simulate) => "a simulate",
            Some(Cause::Indirect) => "an unknown callee",
            Some(Cause::Callback { .. }) => "a callback builtin",
        };
        *tally.entry(cause).or_default() += 1;
    }
    for (cause, count) in &tally {
        println!("    {count:>4}  {cause}");
    }

    assert!(
        !regions.is_empty(),
        "the examples open no region, so the census measured nothing"
    );
}

/// The module that decides nothing about itself: byte-identical in both programs below, naming
/// nothing outside itself.
const UNCHANGED: &str = r#"
fn go(f: (Int) -> Int) -> Int =
  with_cell[acc](0) { c -> { cell_set(c, f(1)); cell_get(c) } }
"#;

/// A second module the first neither names nor reaches.
const ELSEWHERE: &str = r#"
effect amb { read flip[coin]() -> Bool }

fn search() -> Int =
  handle { if amb.flip[coin]() { 1 } else { 2 } } with {
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  }
"#;

#[track_caller]
fn regions_over(modules: &[(&str, &str)]) -> Regions {
    let mut map = SourceMap::new();
    let ids: Vec<SourceId> = modules
        .iter()
        .map(|(name, src)| map.add(format!("{name}.ply"), src.to_string()))
        .collect();
    let inputs: Vec<_> = ids
        .iter()
        .zip(modules)
        .map(|(&id, (name, src))| (id, ModuleName::from_dotted(name), *src))
        .collect();
    let mut program = match parse_program(inputs) {
        Ok(p) => p,
        Err(ds) => panic!("the probe must parse: {ds:#?}"),
    };
    let resolved = resolve(&mut program).expect("the probe must resolve");
    infer(&program, &resolved)
}

/// **A region's kind may not be cached under its own definition's hash.**
#[test]
fn a_capture_in_an_unrelated_module_makes_a_region_shared() {
    let alone = regions_over(&[("unchanged", UNCHANGED)]);
    assert_eq!(
        region(&alone, "acc").kind,
        RegionKind::Unique,
        "with no capture written anywhere, an unknown callee reaches none"
    );

    let together = regions_over(&[("unchanged", UNCHANGED), ("elsewhere", ELSEWHERE)]);
    assert_eq!(
        region(&together, "acc").kind,
        RegionKind::Shared,
        "`unchanged` is byte-identical and names nothing in `elsewhere`, yet its region's kind is \
         a function of `elsewhere` — so this decision cannot be filed under `go`'s hash"
    );
    assert!(
        matches!(
            &region(&together, "acc")
                .capture
                .as_ref()
                .expect("a shared region names its site")
                .cause,
            Cause::Indirect
        ),
        "{:?}",
        region(&together, "acc").capture
    );
}
