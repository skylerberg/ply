//! A program's checked declarations written out by hand: the effects it declares and each
//! definition's row, which is all a binding reads of a program.

use ply_eval::{
    Analysis, CheckOutput, DefInfo, EffectAtom, EffectInfo, Footprint, Mode, ModuleInfo,
    ModuleName, OpInfo, Resource, SourceId, SourceMap, Span, Symbol, TestInfo,
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
    /// `module` declares `effect`, each operation `(name, mode, whether it takes a resource)`.
    pub fn effect(mut self, module: &str, effect: &str, nondet: bool, ops: &[(&str, Mode, bool)]) -> Sketch {
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

    /// `module` declares the test `label`, which performs nothing.
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

    /// The program loaded as a machine holds one: its modules' texts under their sources.
    pub fn loaded(self) -> ply_machine::load::Loaded {
        let mut sources = SourceMap::new();
        let files = self
            .texts
            .iter()
            .map(|(path, text)| {
                sources.add(path, text.as_str());
                ply_machine::load::Found {
                    path: path.into(),
                }
            })
            .collect();
        let front = Analysis {
            check: self.check.clone(),
            ..Analysis::default()
        };
        ply_machine::load::Loaded {
            root: ".".into(),
            files,
            sources,
            front: std::sync::Arc::new(front),
            check: self.check,
            hashes: Default::default(),
            frontend: Default::default(),
        }
    }
}

/// The atom `effect.mode[resource]`, a singleton's where `resource` is empty.
pub fn atom(effect: &str, resource: &str, mode: Mode) -> EffectAtom {
    let resource = if resource.is_empty() {
        Resource::Singleton
    } else {
        Resource::Named(Symbol::new(resource))
    };
    EffectAtom::new(effect, resource, mode)
}
