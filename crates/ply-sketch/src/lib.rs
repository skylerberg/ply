//! A program's checked declarations written out by hand, for the Rust tests of what reads them: the
//! effects it declares, each definition's row, its tests and its modules' texts. A binding, a
//! machine and a loader read no more of a program than this, so their tests compile none.

use ply_eval::{
    Analysis, CheckOutput, DefHash, DefInfo, EffectAtom, EffectInfo, Footprint, Mode, ModuleInfo,
    ModuleName, OpInfo, Resource, SourceId, Span, Symbol, TestInfo,
};

#[derive(Default)]
pub struct Sketch {
    check: CheckOutput,
    /// Each module's path and text, its source the one its place here names.
    texts: Vec<(String, String)>,
}

/// `name` as the module `module` declares it program-wide; the language's own are under no module.
fn qualified(module: &str, name: &str) -> String {
    if module.is_empty() {
        name.to_string()
    } else {
        format!("{module}.{name}")
    }
}

impl Sketch {
    /// A program declaring nothing of its own: the effects every program is checked with, as the
    /// compiler's `prelude_effects` declares them.
    pub fn program() -> Sketch {
        Sketch::default()
            .effect(
                "",
                "task",
                true,
                &[
                    ("spawn", Mode::Write, false),
                    ("join", Mode::Write, false),
                    ("yield", Mode::Write, false),
                    ("cancel", Mode::Write, false),
                    ("await", Mode::Write, false),
                    ("channel", Mode::Write, false),
                    ("send", Mode::Write, false),
                    ("recv", Mode::Write, false),
                    ("close", Mode::Write, false),
                    ("select", Mode::Write, false),
                ],
            )
            .effect(
                "",
                "clock",
                true,
                &[("now", Mode::Read, false), ("sleep", Mode::Write, false)],
            )
            .effect(
                "",
                "random",
                true,
                &[("next", Mode::Write, false), ("below", Mode::Write, false)],
            )
            .effect("", "sim", false, &[("seed", Mode::Read, false)])
            .effect("", "abort", false, &[("raise", Mode::Raise, false)])
            .effect("", "diverges", false, &[])
    }

    /// `module` declares `effect`, each operation `(name, mode, whether it takes a resource)`.
    pub fn effect(
        mut self,
        module: &str,
        effect: &str,
        nondet: bool,
        ops: &[(&str, Mode, bool)],
    ) -> Sketch {
        let name = Symbol::new(qualified(module, effect));
        let ops = ops
            .iter()
            .map(|(op, mode, resource)| {
                let info = OpInfo {
                    name: Symbol::new(*op),
                    mode: *mode,
                    resource_param: *resource,
                    span: Span::DUMMY,
                    params: Vec::new(),
                };
                (Symbol::new(*op), info)
            })
            .collect();
        let info = EffectInfo {
            name: name.clone(),
            module: ModuleName::from_dotted(module),
            simple_name: Symbol::new(effect),
            nondet,
            ops,
            span: Span::DUMMY,
        };
        self.check.effects.insert(name, info);
        self
    }

    /// `module` defines `name`, its published row `atoms`, each definition at a place of its own.
    pub fn def(mut self, module: &str, name: &str, atoms: Vec<EffectAtom>) -> Sketch {
        let full = Symbol::new(qualified(module, name));
        let footprint = Footprint::from_atoms(atoms);
        let at = self.check.defs.len() as u32 * 10;
        let info = DefInfo {
            name: full.clone(),
            module: ModuleName::from_dotted(module),
            simple_name: Symbol::new(name),
            footprint: footprint.clone(),
            performed: footprint,
            span: Span::new(SourceId(0), at, at + 5),
        };
        self.check.defs.insert(full, info);
        self
    }

    /// The module `name` at `path`, holding `text`.
    pub fn module(mut self, name: &str, path: &str, text: &str) -> Sketch {
        let source = SourceId(self.texts.len() as u32);
        self.texts.push((path.to_string(), text.to_string()));
        let info = ModuleInfo {
            name: ModuleName::from_dotted(name),
            source,
            items: Vec::new(),
            imports: Vec::new(),
        };
        self.check.modules.insert(Symbol::new(name), info);
        self
    }

    /// `module` declares a deterministic test labelled `label`, which performs nothing, after every
    /// test before it.
    pub fn test(mut self, module: &str, label: &str) -> Sketch {
        let info = TestInfo {
            name: label.to_string(),
            module: ModuleName::from_dotted(module),
            key: Symbol::new(qualified(module, label)),
            index: self.check.tests.len(),
            nondet: false,
            footprint: Footprint::empty(),
            span: Span::DUMMY,
        };
        self.check.tests.push(info);
        self
    }

    pub fn check(self) -> CheckOutput {
        self.check
    }

    /// The checked declarations and each module's path and text, in source order.
    pub fn into_parts(self) -> (CheckOutput, Vec<(String, String)>) {
        (self.check, self.texts)
    }

    /// The front end's answer over this program, which a backend built from `program` describes.
    pub fn analysis(self, program: &[u8]) -> Analysis {
        Analysis {
            check: self.check,
            hashes_digest: DefHash::of(program),
            ..Analysis::default()
        }
    }
}

/// The atom `effect.mode[resource]`, a singleton's where `resource` is empty.
pub fn atom(effect: &str, resource: &str, mode: Mode) -> EffectAtom {
    EffectAtom::new(effect, resource_of(resource), mode)
}

/// The atom naming the one operation `op`, `effect.op[resource]`.
pub fn operation(effect: &str, resource: &str, mode: Mode, op: &str) -> EffectAtom {
    EffectAtom::operation(effect, resource_of(resource), mode, op)
}

fn resource_of(resource: &str) -> Resource {
    if resource.is_empty() {
        Resource::Singleton
    } else {
        Resource::Named(Symbol::new(resource))
    }
}
