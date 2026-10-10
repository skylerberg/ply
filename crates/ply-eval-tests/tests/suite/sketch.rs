//! A program's checked declarations written out by hand: the effects it declares, the language's
//! among them, and each definition's row, which is all a binding reads of a program.

use ply_eval::{
    Analysis, CheckOutput, DefHash, DefInfo, EffectAtom, EffectInfo, Footprint, Mode, ModuleName,
    OpInfo, Resource, SourceId, Span, Symbol, TestInfo,
};

#[derive(Default)]
pub struct Sketch(CheckOutput);

/// `name` as the module `module` declares it program-wide; the language's own are under no module.
fn qualified(module: &str, name: &str) -> String {
    if module.is_empty() {
        name.to_string()
    } else {
        format!("{module}.{name}")
    }
}

impl Sketch {
    /// A program declaring nothing of its own: the effects every program is checked with.
    pub fn program() -> Sketch {
        Sketch::default()
            .effect(
                "",
                "task",
                false,
                &[
                    ("spawn", Mode::Write, false),
                    ("join", Mode::Write, false),
                    ("yield", Mode::Write, false),
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
        self.0.effects.insert(name, info);
        self
    }

    /// `module` defines `name`, its published row `atoms`, each definition at a place of its own.
    pub fn def(mut self, module: &str, name: &str, atoms: Vec<EffectAtom>) -> Sketch {
        let full = Symbol::new(qualified(module, name));
        let footprint = Footprint::from_atoms(atoms);
        let at = self.0.defs.len() as u32 * 10;
        let info = DefInfo {
            name: full.clone(),
            module: ModuleName::from_dotted(module),
            simple_name: Symbol::new(name),
            footprint: footprint.clone(),
            performed: footprint,
            span: Span::new(SourceId(0), at, at + 5),
        };
        self.0.defs.insert(full, info);
        self
    }

    /// `module` declares a deterministic test labelled `label`, after every test before it.
    pub fn test(mut self, module: &str, label: &str) -> Sketch {
        let index = self.0.tests.len();
        self.0.tests.push(TestInfo {
            name: label.to_string(),
            module: ModuleName::from_dotted(module),
            key: Symbol::new(qualified(module, label)),
            index,
            nondet: false,
            footprint: Footprint::default(),
            span: Span::DUMMY,
        });
        self
    }

    pub fn check(self) -> CheckOutput {
        self.0
    }

    /// The front end's answer over this program, which a backend built from `program` describes.
    pub fn analysis(self, program: &[u8]) -> Analysis {
        Analysis {
            check: self.0,
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
