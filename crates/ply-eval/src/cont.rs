//! The explicit control stack, and the delimited continuations cut out of it.
//!
//! The stack is a list of **segments**. A segment is a persistent list of
//! frames sitting on top of the `handle` that delimits it; the outermost
//! segment has no delimiter. Capturing a continuation is taking the segments
//! down to and including the segment whose prompt matches — a `Vec` of two
//! pointers each, one entry per enclosing handler crossed, and never one entry
//! per pending frame. That is what makes a second resumption cost the same as
//! the first, and it is the reason multi-shot is affordable rather than
//! theoretical.
//!
//! Resuming pushes those segments back onto whatever stack is current. Because
//! the captured slice carries its own prompt, the handler is reinstalled by the
//! act of resuming: handlers are **deep**, and a clause runs on the stack
//! *below* its own handler so that a clause performing the operation it handles
//! reaches the next handler out rather than itself.

use crate::code::{Clause, Code, ReturnArm, Stmt};
use crate::env::Env;
use crate::value::{Value, Vector};
use ply_span::{Span, Symbol};
use ply_syntax::ast::{BinOp, Ident, UnOp};
use rpds::List;
use std::rc::Rc;

/// One suspended step. Every variant names the value it is waiting for and what
/// it will do with it; together they cover `NodeKind` exhaustively, plus the
/// three prelude builtins that call back into user code.
#[derive(Clone)]
pub enum Frame {
    Unary {
        op: UnOp,
        operand_span: Span,
        span: Span,
    },

    /// Waiting for the left operand; the right one is still code.
    BinaryRhs {
        op: BinOp,
        rhs: Code,
        env: Env,
        module: usize,
        lhs_span: Span,
        span: Span,
    },

    /// Waiting for the right operand, holding the evaluated left one.
    BinaryApply {
        op: BinOp,
        lhs: Value,
        lhs_span: Span,
        rhs_span: Span,
        span: Span,
    },

    /// `&&` and `||`: the left operand decided nothing, so evaluate the right.
    ShortCircuit {
        op: BinOp,
        rhs: Code,
        env: Env,
        module: usize,
        rhs_span: Span,
    },

    AppCallee {
        args: Rc<Vec<Code>>,
        env: Env,
        module: usize,
        span: Span,
    },

    /// Waiting for `args[next - 1]`, holding the callee and the arguments
    /// already evaluated.
    AppArgs {
        callee: Value,
        done: Vec<Value>,
        args: Rc<Vec<Code>>,
        next: usize,
        env: Env,
        module: usize,
        span: Span,
    },

    /// A user function's body is running. It transforms nothing: every frame
    /// that holds pending code carries its own module, so returning from a call
    /// restores the caller's scope with no help. It exists to bound recursion
    /// and to give the causal-slice tracer its enter and exit events.
    ///
    /// Every call pushes one, tail position included: eliding it for a tail
    /// call leaves a tail-recursive runaway unbounded here while the
    /// tree-walker diagnoses it in milliseconds.
    Call {
        name: Option<Symbol>,
        call_site: Span,
    },

    /// Hands the value it receives to a captured continuation. This is the
    /// whole of the tail-resumptive clause: `op(x) -> e` is `op(x) resume k ->
    /// k(e)`, and this frame is the `k(_)`.
    Resume {
        k: Rc<Continuation>,
    },

    If {
        then_branch: Code,
        else_branch: Code,
        env: Env,
        module: usize,
        cond_span: Span,
    },

    /// Waiting for the scrutinee. `next` is the first arm not yet tried, so the
    /// same frame serves the initial dispatch and a guard that failed.
    MatchArms {
        scrutinee: Value,
        arms: Rc<Vec<crate::code::Arm>>,
        next: usize,
        env: Env,
        module: usize,
        scrutinee_span: Span,
    },

    /// Waiting for an arm's guard, holding the bindings that arm's pattern made
    /// so a failing guard can fall through without rematching.
    MatchGuard {
        scrutinee: Value,
        arms: Rc<Vec<crate::code::Arm>>,
        at: usize,
        arm_env: Env,
        env: Env,
        module: usize,
        scrutinee_span: Span,
    },

    /// Waiting for `stmts[next - 1]`. `scope` accumulates `let` bindings.
    BlockStep {
        stmts: Rc<Vec<Stmt>>,
        next: usize,
        tail: Option<Code>,
        scope: Env,
        module: usize,
    },

    RecordField {
        done: Vec<(Symbol, Value)>,
        fields: Rc<Vec<(Symbol, Code)>>,
        next: usize,
        env: Env,
        module: usize,
    },

    FieldAccess {
        field: Ident,
        base_span: Span,
    },

    ListItem {
        done: Vec<Value>,
        items: Rc<Vec<Code>>,
        next: usize,
        env: Env,
        module: usize,
    },

    /// Waiting for `args[next - 1]` of a `perform`. When the last one lands the
    /// machine moves to the `Perform` state rather than pushing another frame.
    PerformArgs {
        effect: Symbol,
        op: Symbol,
        resource: Option<Symbol>,
        done: Vec<Value>,
        args: Rc<Vec<Code>>,
        next: usize,
        env: Env,
        module: usize,
        span: Span,
    },

    /// The cell is allocated only once the initial value lands.
    WithCellBody {
        resource: Symbol,
        binder: Symbol,
        body: Code,
        env: Env,
        module: usize,
    },

    /// `map`, `filter` and `fold` call user code, so their loops are frames
    /// rather than host recursion — otherwise a continuation captured inside the
    /// function passed to `map` would be captured across a native frame that
    /// cannot be re-entered.
    MapStep {
        f: Value,
        items: Vector<Value>,
        next: usize,
        done: Vec<Value>,
        span: Span,
    },

    FilterStep {
        f: Value,
        items: Vector<Value>,
        next: usize,
        done: Vec<Value>,
        span: Span,
    },

    FoldStep {
        f: Value,
        items: Vector<Value>,
        next: usize,
        span: Span,
    },
}

pub struct Prompt {
    pub clauses: Rc<Vec<Clause>>,
    /// Each clause's effect under its program-wide name, resolved where the
    /// `handle` was written. A perform reached from another module spells the
    /// same effect differently and the two only meet once both are qualified.
    pub effects: Rc<Vec<Symbol>>,
    pub ret: Option<Rc<ReturnArm>>,
    pub env: Env,
    pub module: usize,
    pub span: Span,
}

impl Prompt {
    /// The index of the clause handling this operation, innermost clause order.
    /// A clause without a resource label handles every resource of its
    /// operation; an operation declared without `[r]` has exactly one anyway.
    pub fn clause_for(
        &self,
        effect: &Symbol,
        op: &Symbol,
        resource: Option<&Symbol>,
    ) -> Option<usize> {
        self.clauses
            .iter()
            .zip(self.effects.iter())
            .position(|(c, e)| {
                e == effect
                    && c.op == *op
                    && match (&c.resource, resource) {
                        (None, _) => true,
                        (Some(cr), Some(r)) => cr == r,
                        (Some(_), None) => false,
                    }
            })
    }
}

#[derive(Clone, Default)]
pub struct Segment {
    frames: List<Frame>,
    prompt: Option<Rc<Prompt>>,
    calls: usize,
}

impl Segment {
    pub fn base() -> Segment {
        Segment::default()
    }

    pub fn under(prompt: Rc<Prompt>) -> Segment {
        Segment {
            frames: List::new(),
            prompt: Some(prompt),
            calls: 0,
        }
    }

    pub fn prompt(&self) -> Option<&Rc<Prompt>> {
        self.prompt.as_ref()
    }

    pub fn frames(&self) -> usize {
        self.frames.len()
    }

    pub fn calls(&self) -> usize {
        self.calls
    }
}

fn is_call(frame: &Frame) -> usize {
    usize::from(matches!(frame, Frame::Call { .. }))
}

/// What the machine does with the value it is currently returning.
pub enum Next {
    Frame(Frame, Stack),
    /// The delimited body finished. Apply this handler's `return` clause, if
    /// any, and carry on with the stack the handler was installed on.
    Leave(Rc<Prompt>, Stack),
    Done,
}

pub struct Handled {
    /// How many segments to capture, counting from the innermost. Always at
    /// least one: the segment delimited by the handler itself.
    pub segments: usize,
    pub prompt: Rc<Prompt>,
    pub clause: usize,
}

#[derive(Clone)]
pub struct Stack {
    /// Head is the innermost segment. The last one is always the base.
    segments: List<Segment>,
    frames: usize,
    calls: usize,
}

impl Default for Stack {
    fn default() -> Stack {
        Stack::new()
    }
}

impl Stack {
    pub fn new() -> Stack {
        Stack {
            segments: List::new().push_front(Segment::base()),
            frames: 0,
            calls: 0,
        }
    }

    /// Total pending frames. O(1), and it is the resource bound on a stack that
    /// is now a heap value rather than the native one.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Pending calls — the [`Frame::Call`]s among [`Stack::frames`], counted the
    /// same way the tree-walker counts its own nesting. O(1), and it is the
    /// *semantic* bound: the one number both engines answer to, so that a
    /// runaway recursion is the same diagnostic whichever engine ran it.
    pub fn calls(&self) -> usize {
        self.calls
    }

    pub fn segments(&self) -> usize {
        self.segments.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames == 0 && self.segments.len() == 1
    }

    pub fn push(&self, frame: Frame) -> Stack {
        let top = self
            .segments
            .first()
            .expect("a stack always has its base segment");
        let calls = is_call(&frame);
        let replaced = Segment {
            frames: top.frames.push_front(frame),
            prompt: top.prompt.clone(),
            calls: top.calls + calls,
        };
        Stack {
            segments: self
                .segments
                .drop_first()
                .expect("a stack always has its base segment")
                .push_front(replaced),
            frames: self.frames + 1,
            calls: self.calls + calls,
        }
    }

    pub fn push_prompt(&self, prompt: Rc<Prompt>) -> Stack {
        Stack {
            segments: self.segments.push_front(Segment::under(prompt)),
            frames: self.frames,
            calls: self.calls,
        }
    }

    pub fn prompt(&self) -> Option<&Rc<Prompt>> {
        self.segments.first().and_then(Segment::prompt)
    }

    pub fn next(&self) -> Next {
        let top = self
            .segments
            .first()
            .expect("a stack always has its base segment");
        if let Some(frame) = top.frames.first() {
            let calls = is_call(frame);
            let rest = Segment {
                frames: top
                    .frames
                    .drop_first()
                    .expect("a non-empty list drops its head"),
                prompt: top.prompt.clone(),
                calls: top.calls - calls,
            };
            let stack = Stack {
                segments: self
                    .segments
                    .drop_first()
                    .expect("a stack always has its base segment")
                    .push_front(rest),
                frames: self.frames - 1,
                calls: self.calls - calls,
            };
            return Next::Frame(frame.clone(), stack);
        }
        match &top.prompt {
            Some(prompt) => Next::Leave(
                prompt.clone(),
                Stack {
                    segments: self
                        .segments
                        .drop_first()
                        .expect("a stack always has its base segment"),
                    frames: self.frames,
                    calls: self.calls,
                },
            ),
            None => Next::Done,
        }
    }

    pub fn find_handler(
        &self,
        effect: &Symbol,
        op: &Symbol,
        resource: Option<&Symbol>,
    ) -> Option<Handled> {
        for (depth, segment) in self.segments.iter().enumerate() {
            let Some(prompt) = segment.prompt() else {
                continue;
            };
            if let Some(clause) = prompt.clause_for(effect, op, resource) {
                return Some(Handled {
                    segments: depth + 1,
                    prompt: prompt.clone(),
                    clause,
                });
            }
        }
        None
    }

    /// Cuts the innermost `segments` segments away. The returned stack is what
    /// the handler clause runs on; the continuation is what resuming puts back.
    pub fn capture(&self, segments: usize) -> (Continuation, Stack) {
        let mut taken = Vec::with_capacity(segments);
        let mut rest = self.clone();
        let mut frames = 0;
        let mut calls = 0;
        for _ in 0..segments {
            let top = rest
                .segments
                .first()
                .expect("capture never crosses the base segment")
                .clone();
            frames += top.frames();
            calls += top.calls();
            rest = Stack {
                segments: rest
                    .segments
                    .drop_first()
                    .expect("capture never crosses the base segment"),
                frames: rest.frames - top.frames(),
                calls: rest.calls - top.calls(),
            };
            taken.push(top);
        }
        (
            Continuation {
                segments: Rc::new(taken),
                frames,
                calls,
            },
            rest,
        )
    }

    /// Splices a captured continuation on top of this stack. Every resumption
    /// of one continuation splices the same shared segments, so the second
    /// costs what the first did.
    pub fn resume(&self, k: &Continuation) -> Stack {
        let mut out = self.clone();
        for segment in k.segments.iter().rev() {
            out = Stack {
                segments: out.segments.push_front(segment.clone()),
                frames: out.frames + segment.frames(),
                calls: out.calls + segment.calls(),
            };
        }
        out
    }
}

/// A delimited continuation: the control captured at a `perform`, from the
/// perform site down to and including the handler that answered it.
///
/// It captures **control only**. The world is threaded through the machine
/// rather than snapshotted here, so a resumption observes the handler's state
/// as of the call to `resume`.
pub struct Continuation {
    /// Innermost first — the order `capture` produced and the reverse of the
    /// order `resume` pushes them back.
    segments: Rc<Vec<Segment>>,
    frames: usize,
    calls: usize,
}

impl Continuation {
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// What splicing this back costs against the call budget: a resumption
    /// re-installs the calls the capture cut away.
    pub fn calls(&self) -> usize {
        self.calls
    }

    pub fn segments(&self) -> usize {
        self.segments.len()
    }
}

impl Clone for Continuation {
    fn clone(&self) -> Continuation {
        Continuation {
            segments: Rc::clone(&self.segments),
            frames: self.frames,
            calls: self.calls,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_span::Span;

    fn frame(n: i64) -> Frame {
        Frame::FieldAccess {
            field: Ident::new(format!("f{n}"), Span::DUMMY),
            base_span: Span::DUMMY,
        }
    }

    fn field_of(f: &Frame) -> String {
        match f {
            Frame::FieldAccess { field, .. } => field.name.to_string(),
            _ => panic!("expected a field frame"),
        }
    }

    fn prompt() -> Rc<Prompt> {
        Rc::new(Prompt {
            clauses: Rc::new(Vec::new()),
            effects: Rc::new(Vec::new()),
            ret: None,
            env: Env::empty(),
            module: 0,
            span: Span::DUMMY,
        })
    }

    #[test]
    fn a_new_stack_is_done_immediately() {
        assert!(matches!(Stack::new().next(), Next::Done));
        assert!(Stack::new().is_empty());
    }

    #[test]
    fn frames_come_back_innermost_first() {
        let s = Stack::new().push(frame(1)).push(frame(2));
        assert_eq!(s.frames(), 2);
        let Next::Frame(top, rest) = s.next() else {
            panic!("expected a frame");
        };
        assert_eq!(field_of(&top), "f2");
        let Next::Frame(under, rest) = rest.next() else {
            panic!("expected a frame");
        };
        assert_eq!(field_of(&under), "f1");
        assert!(matches!(rest.next(), Next::Done));
    }

    #[test]
    fn popping_a_frame_leaves_the_original_stack_intact() {
        let s = Stack::new().push(frame(1));
        let _ = s.next();
        assert_eq!(s.frames(), 1);
    }

    #[test]
    fn an_exhausted_segment_yields_its_prompt_and_then_the_stack_under_it() {
        let s = Stack::new().push(frame(1)).push_prompt(prompt());
        let Next::Leave(_, under) = s.next() else {
            panic!("expected to leave the segment");
        };
        assert_eq!(under.segments(), 1);
        assert_eq!(under.frames(), 1);
    }

    #[test]
    fn capture_takes_the_segments_above_and_including_the_handler() {
        let s = Stack::new()
            .push(frame(0))
            .push_prompt(prompt())
            .push(frame(1))
            .push(frame(2));
        assert_eq!(s.segments(), 2);

        let (k, below) = s.capture(1);
        assert_eq!(k.frames(), 2);
        assert_eq!(k.segments(), 1);
        assert_eq!(below.segments(), 1);
        assert_eq!(below.frames(), 1);
    }

    #[test]
    fn resuming_reinstalls_the_handler_that_delimited_the_capture() {
        let s = Stack::new().push_prompt(prompt()).push(frame(1));
        let (k, below) = s.capture(1);
        assert!(below.prompt().is_none());

        let resumed = below.resume(&k);
        assert!(resumed.prompt().is_some());
        assert_eq!(resumed.frames(), 1);
    }

    #[test]
    fn a_continuation_may_be_resumed_twice_onto_different_stacks() {
        let s = Stack::new().push_prompt(prompt()).push(frame(9));
        let (k, below) = s.capture(1);

        let once = below.resume(&k);
        let twice = below.push(frame(5)).resume(&k);

        assert_eq!(once.frames(), 1);
        assert_eq!(twice.frames(), 2);

        let Next::Frame(a, _) = once.next() else {
            panic!("expected a frame");
        };
        let Next::Frame(b, _) = twice.next() else {
            panic!("expected a frame");
        };
        assert_eq!(field_of(&a), "f9");
        assert_eq!(field_of(&b), "f9");
    }

    #[test]
    fn capture_crosses_every_handler_between_the_perform_and_its_own() {
        let s = Stack::new()
            .push_prompt(prompt())
            .push(frame(1))
            .push_prompt(prompt())
            .push(frame(2))
            .push_prompt(prompt())
            .push(frame(3));

        let (k, below) = s.capture(3);
        assert_eq!(k.segments(), 3);
        assert_eq!(k.frames(), 3);
        assert_eq!(below.segments(), 1);
        assert_eq!(below.frames(), 0);

        assert_eq!(below.resume(&k).segments(), 4);
    }

    #[test]
    fn find_handler_reports_the_innermost_matching_prompt() {
        let effect = Symbol::new("db");
        let op = Symbol::new("get");
        let clause = |resource: Option<&str>| Clause {
            effect: ply_syntax::ast::QName::bare(Ident::new("db", Span::DUMMY)),
            op: op.clone(),
            resource: resource.map(Symbol::new),
            params: Rc::new(Vec::new()),
            resume: None,
            body: crate::code::lower(&crate::build::int(0)),
            span: Span::DUMMY,
        };
        let with = |c: Clause| {
            Rc::new(Prompt {
                clauses: Rc::new(vec![c]),
                effects: Rc::new(vec![effect.clone()]),
                ret: None,
                env: Env::empty(),
                module: 0,
                span: Span::DUMMY,
            })
        };

        let s = Stack::new()
            .push_prompt(with(clause(Some("users"))))
            .push_prompt(with(clause(Some("orders"))));

        let users = Symbol::new("users");
        let found = s
            .find_handler(&effect, &op, Some(&users))
            .expect("the outer handler matches");
        assert_eq!(found.segments, 2);
        assert_eq!(found.clause, 0);

        let orders = Symbol::new("orders");
        let inner = s
            .find_handler(&effect, &op, Some(&orders))
            .expect("the inner handler matches");
        assert_eq!(inner.segments, 1);
    }

    #[test]
    fn an_unhandled_operation_finds_no_prompt() {
        let s = Stack::new().push_prompt(prompt());
        assert!(
            s.find_handler(&Symbol::new("db"), &Symbol::new("get"), None)
                .is_none()
        );
    }
}
