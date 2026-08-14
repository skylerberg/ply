//! What the machine does with a value it is handing back to the stack.
//!
//! One arm per [`Frame`]: every frame names the value it was waiting for and
//! what it does next, so this is the whole of the `Return` half of ADR 0005
//! §1.3. Nothing here recurses — an arm either produces a value or moves the
//! machine to its next state — which is what keeps a Ply call off the native
//! stack.

use crate::builtins::advance;
use crate::code::Stmt as CodeStmt;
use crate::cont::Frame;
use crate::handler;
use crate::interp::{err_let_mismatch, err_no_such_field, strict_binary};
use crate::machine::{Machine, apply_unary, short_circuits};
use crate::value::{Value, type_error};
use ply_span::{Diagnostic, Symbol};
use ply_syntax::ast::BinOp;
use std::collections::BTreeMap;
use std::sync::Arc;

impl Machine<'_> {
    pub(crate) fn dispatch(&mut self, frame: Frame, value: Value) -> Result<(), Diagnostic> {
        match frame {
            Frame::Unary {
                op,
                operand_span,
                span,
            } => {
                let out = apply_unary(op, &value, operand_span, span)?;
                self.go_return(out);
            }

            Frame::BinaryRhs {
                op,
                rhs,
                env,
                module,
                lhs_span,
                span,
            } => {
                let rhs_span = rhs.span;
                if let BinOp::And | BinOp::Or = op {
                    let lhs = value.as_bool(lhs_span, "a logical operator")?;
                    if short_circuits(op, lhs) {
                        self.go_return(Value::Bool(lhs));
                        return Ok(());
                    }
                    self.push(
                        Frame::ShortCircuit {
                            op,
                            rhs: rhs.clone(),
                            env: env.clone(),
                            module,
                            rhs_span,
                        },
                        rhs_span,
                    )?;
                } else {
                    self.push(
                        Frame::BinaryApply {
                            op,
                            lhs: value,
                            lhs_span,
                            rhs_span,
                            span,
                        },
                        span,
                    )?;
                }
                self.go_eval(rhs, env, module);
            }

            Frame::BinaryApply {
                op,
                lhs,
                lhs_span,
                rhs_span,
                span,
            } => {
                let out = strict_binary(op, &lhs, &value, lhs_span, rhs_span, span)?;
                self.go_return(out);
            }

            Frame::ShortCircuit { rhs_span, .. } => {
                let rhs = value.as_bool(rhs_span, "a logical operator")?;
                self.go_return(Value::Bool(rhs));
            }

            Frame::AppCallee {
                args,
                env,
                module,
                span,
            } => {
                if args.is_empty() {
                    return self.apply(value, Vec::new(), span);
                }
                let first = args[0].clone();
                self.push(
                    Frame::AppArgs {
                        callee: value,
                        done: Vec::with_capacity(args.len()),
                        args,
                        next: 1,
                        env: env.clone(),
                        module,
                        span,
                    },
                    span,
                )?;
                self.go_eval(first, env, module);
            }

            Frame::AppArgs {
                callee,
                mut done,
                args,
                next,
                env,
                module,
                span,
            } => {
                done.push(value);
                match args.get(next) {
                    None => return self.apply(callee, done, span),
                    Some(arg) => {
                        let arg = arg.clone();
                        self.push(
                            Frame::AppArgs {
                                callee,
                                done,
                                args,
                                next: next + 1,
                                env: env.clone(),
                                module,
                                span,
                            },
                            span,
                        )?;
                        self.go_eval(arg, env, module);
                    }
                }
            }

            Frame::Call { .. } => self.go_return(value),

            Frame::Resume { k } => return self.resume_continuation(&k, value),

            Frame::If {
                then_branch,
                else_branch,
                env,
                module,
                cond_span,
            } => {
                let taken = if value.as_bool(cond_span, "an `if` condition")? {
                    then_branch
                } else {
                    else_branch
                };
                self.go_eval(taken, env, module);
            }

            Frame::MatchArms {
                scrutinee,
                arms,
                next,
                env,
                module,
                scrutinee_span,
            } => {
                let scrutinee = if next == 0 { value } else { scrutinee };
                return self.try_arms(scrutinee, arms, next, env, module, scrutinee_span);
            }

            Frame::MatchGuard {
                scrutinee,
                arms,
                at,
                arm_env,
                env,
                module,
                scrutinee_span,
            } => {
                let guard_span = arms[at]
                    .guard
                    .as_ref()
                    .map_or(scrutinee_span, |guard| guard.span);
                if value.as_bool(guard_span, "a match guard")? {
                    let body = arms[at].body.clone();
                    self.go_eval(body, arm_env, module);
                    return Ok(());
                }
                // A rejected arm falls through to the next one with the
                // scrutinee it was already handed, never by rematching.
                self.push(
                    Frame::MatchArms {
                        scrutinee,
                        arms,
                        next: at + 1,
                        env,
                        module,
                        scrutinee_span,
                    },
                    scrutinee_span,
                )?;
                self.go_return(Value::Unit);
            }

            Frame::BlockStep {
                stmts,
                next,
                tail,
                scope,
                module,
            } => {
                let mut scope = scope;
                if let CodeStmt::Let { pat, span, .. } = &stmts[next - 1] {
                    let mut bound = scope.clone();
                    if !self.match_pattern(pat, &value, &mut bound, module)? {
                        return Err(err_let_mismatch(*span, &value));
                    }
                    scope = bound;
                }
                return self.enter_block(stmts, next, tail, scope, module);
            }

            Frame::RecordField {
                mut done,
                fields,
                next,
                env,
                module,
            } => {
                done.push((fields[next - 1].0.clone(), value));
                match fields.get(next) {
                    None => {
                        let map: BTreeMap<Symbol, Value> = done.into_iter().collect();
                        self.go_return(Value::Record(Arc::new(map)));
                    }
                    Some((_, code)) => {
                        let code = code.clone();
                        self.push(
                            Frame::RecordField {
                                done,
                                fields,
                                next: next + 1,
                                env: env.clone(),
                                module,
                            },
                            code.span,
                        )?;
                        self.go_eval(code, env, module);
                    }
                }
            }

            Frame::FieldAccess { field, base_span } => match &value {
                Value::Record(fields) => match fields.get(&field.name) {
                    Some(v) => self.go_return(v.clone()),
                    None => return Err(err_no_such_field(&field, fields)),
                },
                other => {
                    return Err(type_error(base_span, "field access", "a record", other));
                }
            },

            Frame::ListItem {
                mut done,
                items,
                next,
                env,
                module,
            } => {
                done.push(value);
                match items.get(next) {
                    None => self.go_return(Value::list(done)),
                    Some(item) => {
                        let item = item.clone();
                        self.push(
                            Frame::ListItem {
                                done,
                                items,
                                next: next + 1,
                                env: env.clone(),
                                module,
                            },
                            item.span,
                        )?;
                        self.go_eval(item, env, module);
                    }
                }
            }

            Frame::PerformArgs {
                effect,
                op,
                resource,
                mut done,
                args,
                next,
                env,
                module,
                span,
            } => {
                done.push(value);
                let transition = handler::perform_args(
                    self.stack(),
                    &effect,
                    &op,
                    &resource,
                    done,
                    &args,
                    next,
                    &env,
                    module,
                    span,
                );
                return self.take(transition);
            }

            Frame::WithCellBody {
                binder,
                body,
                env,
                module,
                ..
            } => {
                let stack = self.stack().clone();
                let transition = handler::open_cell(
                    self.world_mut(),
                    &binder,
                    &body,
                    &env,
                    module,
                    value,
                    stack,
                )?;
                self.record_alloc_access();
                return self.take(transition);
            }

            step @ (Frame::MapStep { .. } | Frame::FilterStep { .. } | Frame::FoldStep { .. }) => {
                let span = builtin_step_span(&step);
                let next = advance(step, value)?;
                return self.run_builtin_step(next, span);
            }
        }
        Ok(())
    }
}

fn builtin_step_span(frame: &Frame) -> ply_span::Span {
    match frame {
        Frame::MapStep { span, .. }
        | Frame::FilterStep { span, .. }
        | Frame::FoldStep { span, .. } => *span,
        _ => ply_span::Span::DUMMY,
    }
}
