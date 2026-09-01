//! What the machine does with a value it is handing back to the stack.

use crate::builtins::advance;
use crate::code::Stmt as CodeStmt;
use crate::cont::Frame;
use crate::handler;
use crate::machine::{Machine, apply_unary, short_circuits};
use crate::semantics::{err_let_mismatch, err_no_such_field, strict_binary};
use crate::value::{Fields, Value, type_error};
use ply_span::{Diagnostic, Symbol};
use ply_syntax::ast::BinOp;
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
                self.go_eval(rhs, module);
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

            Frame::AppCallee { args, module, span } => {
                if args.is_empty() {
                    return self.apply(value, Vec::new(), span);
                }
                let first = args[0].clone();
                crate::rc::note_carry();
                self.push(
                    Frame::AppArgs {
                        callee: value,
                        done: crate::argv::take(args.len()),
                        args,
                        next: 1,
                        module,
                        span,
                    },
                    span,
                )?;
                self.go_eval(first, module);
            }

            Frame::AppArgs {
                callee,
                mut done,
                args,
                next,
                module,
                span,
            } => {
                done.push(value);
                match args.get(next) {
                    None => return self.apply(callee, done, span),
                    Some(arg) => {
                        let arg = arg.clone();
                        crate::rc::note_carry();
                        self.push(
                            Frame::AppArgs {
                                callee,
                                done,
                                args,
                                next: next + 1,
                                module,
                                span,
                            },
                            span,
                        )?;
                        self.go_eval(arg, module);
                    }
                }
            }

            Frame::Call {
                name,
                memo,
                callee_window,
                caller_window,
                ..
            } => {
                self.exit_window(callee_window, caller_window);
                if memo && let Some(name) = &name {
                    self.remember_constant(name, &value);
                }
                self.go_return(value);
            }

            Frame::Exit {
                callee_window,
                caller_window,
            } => {
                self.exit_window(callee_window, caller_window);
                self.go_return(value);
            }

            Frame::Resume { k } => return self.resume_continuation(&k, value),

            Frame::Restore { spill, base_offset } => {
                let windows = self.windows_mut();
                let to = windows.len() - spill as usize;
                windows.truncate(to);
                windows.base = to - base_offset as usize;
                self.go_return(value);
            }

            Frame::If {
                then_branch,
                else_branch,
                module,
                cond_span,
            } => {
                let taken = if value.as_bool(cond_span, "an `if` condition")? {
                    then_branch
                } else {
                    else_branch
                };
                self.go_eval(taken, module);
            }

            Frame::MatchArms {
                scrutinee,
                arms,
                next,
                module,
                scrutinee_span,
            } => {
                let scrutinee = if next == 0 { value } else { scrutinee };
                return self.try_arms(scrutinee, arms, next, module, scrutinee_span);
            }

            Frame::MatchGuard {
                scrutinee,
                arms,
                at,
                module,
                scrutinee_span,
            } => {
                let guard_span = arms[at]
                    .guard
                    .as_ref()
                    .map_or(scrutinee_span, |guard| guard.span);
                if value.as_bool(guard_span, "a match guard")? {
                    let body = arms[at].body.clone();
                    self.go_eval(body, module);
                    return Ok(());
                }
                // A rejected arm falls through to the next one with the scrutinee it was already
                // handed, never by rematching.
                self.push(
                    Frame::MatchArms {
                        scrutinee,
                        arms,
                        next: at + 1,
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
                module,
            } => {
                if let CodeStmt::Let { pat, span, .. } = &stmts[next - 1]
                    && !self.match_pattern(pat, &value, module)?
                {
                    return Err(err_let_mismatch(*span, &value));
                }
                return self.enter_block(stmts, next, tail, module);
            }

            Frame::RecordField {
                mut done,
                fields,
                next,
                module,
            } => {
                done.push((fields[next - 1].0.clone(), value));
                match fields.get(next) {
                    None => {
                        self.go_return(Value::Record(Arc::new(Fields::from_unsorted(done))));
                    }
                    Some((_, code)) => {
                        let code = code.clone();
                        crate::rc::note_carry();
                        self.push(
                            Frame::RecordField {
                                done,
                                fields,
                                next: next + 1,
                                module,
                            },
                            code.span,
                        )?;
                        self.go_eval(code, module);
                    }
                }
            }

            Frame::UpdateField {
                base,
                copies,
                sets,
                mut done,
                next,
                module,
                span,
            } => {
                done.push(value);
                match sets.get(next) {
                    Some((_, code)) => {
                        let code = code.clone();
                        crate::rc::note_carry();
                        self.push(
                            Frame::UpdateField {
                                base,
                                copies,
                                sets,
                                done,
                                next: next + 1,
                                module,
                                span,
                            },
                            span,
                        )?;
                        self.go_eval(code, module);
                    }
                    None => {
                        self.push(
                            Frame::UpdateApply {
                                copies,
                                sets,
                                done,
                                span,
                            },
                            span,
                        )?;
                        self.go_eval(base, module);
                    }
                }
            }

            // The base arrives after the written fields. When the literal names exactly the base's
            // fields — the shape `{..b, f: e}` always has — the record is updated in place if
            // nothing else holds it, and cloned once if something does; a literal that copies
            // fewer fields than the base holds is built as written.
            Frame::UpdateApply {
                copies,
                sets,
                mut done,
                span,
            } => {
                let mut value = value;
                match &mut value {
                    Value::Record(record) => {
                        if let Some(missing) = copies.iter().find(|c| record.get(&c.name).is_none())
                        {
                            return Err(err_no_such_field(missing, record));
                        }
                        let exact = record.len() == copies.len() + sets.len()
                            && sets.iter().all(|(n, _)| record.get(n).is_some());
                        if exact {
                            let fields = Arc::make_mut(record);
                            for ((name, _), v) in sets.iter().zip(done.drain(..)) {
                                fields.set(name, v);
                            }
                            crate::argv::give(done);
                            self.go_return(value);
                        } else {
                            let mut out: Vec<(Symbol, Value)> =
                                Vec::with_capacity(copies.len() + sets.len());
                            for c in copies.iter() {
                                let v = record.get(&c.name).cloned().expect("checked above");
                                out.push((c.name.clone(), v));
                            }
                            for ((name, _), v) in sets.iter().zip(done.drain(..)) {
                                out.push((name.clone(), v));
                            }
                            crate::argv::give(done);
                            self.go_return(Value::Record(Arc::new(Fields::from_unsorted(out))));
                        }
                    }
                    other => {
                        return Err(type_error(span, "field access", "a record", other));
                    }
                }
            }

            Frame::FieldAccess { field, base_span } => {
                let mut value = value;
                match &mut value {
                    // A record whose only owner just handed it over is dying: the projection
                    // moves the field out instead of cloning it, which is what makes `f(s.out)`
                    // with `s` at its last use hand `f` a uniquely-owned value.
                    Value::Record(fields) => {
                        let taken = match Arc::get_mut(fields) {
                            Some(map) => map.set(&field.name, Value::Unit),
                            None => fields.get(&field.name).cloned(),
                        };
                        match taken {
                            Some(v) => self.go_return(v),
                            None => {
                                let Value::Record(fields) = &value else {
                                    unreachable!("just matched a record");
                                };
                                return Err(err_no_such_field(&field, fields));
                            }
                        }
                    }
                    other => {
                        return Err(type_error(base_span, "field access", "a record", other));
                    }
                }
            }

            Frame::ListItem {
                mut done,
                items,
                next,
                module,
            } => {
                done.push(value);
                match items.get(next) {
                    None => self.go_return(Value::list(done)),
                    Some(item) => {
                        let item = item.clone();
                        crate::rc::note_carry();
                        self.push(
                            Frame::ListItem {
                                done,
                                items,
                                next: next + 1,
                                module,
                            },
                            item.span,
                        )?;
                        self.go_eval(item, module);
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
                    module,
                    span,
                );
                return self.take(transition);
            }

            Frame::WithCellBody {
                slot,
                body,
                module,
                region,
                ..
            } => {
                let stack = self.stack().clone();
                let kind = self.region_kind(region);
                let transition = {
                    let (regions, windows) = self.regions_and_windows();
                    handler::open_cell(
                        regions, windows, slot, &body, module, value, stack, kind, region,
                    )?
                };
                self.record_alloc_access();
                return self.take(transition);
            }

            // The region's lexical close.
            Frame::CloseRegion { region } => {
                self.regions_mut().close_region(region);
                // A close moves the shared bump pointer exactly as an allocation does, and two
                // tasks whose closes the search thinks are independent reach two different arenas.
                self.record_alloc_access();
                self.go_return(value);
            }

            Frame::CellUpdateStep { slot, span } => {
                crate::rc::cell_cycle(slot, &value, span);
                if !self.regions_mut().arena_mut().put_back(slot, value) {
                    return Err(crate::builtins::no_such_cell(span, slot));
                }
                self.go_return(Value::Unit);
            }

            step @ (Frame::MapStep { .. }
            | Frame::FilterStep { .. }
            | Frame::FoldStep { .. }
            | Frame::MapFoldStep { .. }
            | Frame::BytesPositionStep { .. }
            | Frame::IterateStep { .. }
            | Frame::MapUpdateStep { .. }) => {
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
        | Frame::FoldStep { span, .. }
        | Frame::MapFoldStep { span, .. }
        | Frame::BytesPositionStep { span, .. }
        | Frame::IterateStep { span, .. }
        | Frame::MapUpdateStep { span, .. } => *span,
        _ => ply_span::Span::DUMMY,
    }
}
