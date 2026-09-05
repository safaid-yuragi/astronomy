//! CFG verification: terminators, branch arguments and dominance (§13, §14).

use crate::error::VerifyError;
use crate::function::Function;
use crate::id::{BlockId, ValueId};
use crate::module::Module;
use crate::value::ValueKind;
use crate::Terminator;

/// Control-flow analysis results for one function.
pub(crate) struct CfgInfo {
    /// Whether each block is reachable from the entry block.
    pub reachable: Vec<bool>,
    /// Reverse post-order number per block; `usize::MAX` when unreachable.
    pub rpo_number: Vec<usize>,
    /// Immediate dominator per block (`None` only for unreachable blocks).
    pub idom: Vec<Option<BlockId>>,
}

impl CfgInfo {
    pub(crate) fn compute(f: &Function) -> CfgInfo {
        let n = f.blocks.len();
        let mut info = CfgInfo {
            reachable: vec![false; n],
            rpo_number: vec![usize::MAX; n],
            idom: vec![None; n],
        };
        if n == 0 {
            return info;
        }

        // Successor lists.
        let successors: Vec<Vec<BlockId>> = f
            .blocks
            .iter()
            .map(|b| {
                let mut succs = Vec::new();
                if let Some(t) = &b.terminator {
                    t.visit_targets(&mut |t| succs.push(t));
                }
                succs
            })
            .collect();

        // Iterative DFS from the entry block, recording post-order.
        // Successors are visited in reverse so that the reverse post-order
        // keeps successors in natural order (`then` before `else`), which
        // the canonical printer relies on (§13.3).
        // Stack entries: (block, next-successor-index).
        let mut post: Vec<BlockId> = Vec::new();
        let mut stack: Vec<(BlockId, usize)> = vec![(BlockId::new(0), 0)];
        info.reachable[0] = true;
        while let Some((block_ref, next)) = stack.last_mut() {
            let block = *block_ref;
            let succs = &successors[block.index()];
            if *next < succs.len() {
                let s = succs[succs.len() - 1 - *next];
                *next += 1;
                if s.index() < n && !info.reachable[s.index()] {
                    info.reachable[s.index()] = true;
                    stack.push((s, 0));
                }
            } else {
                post.push(block);
                stack.pop();
            }
        }

        // RPO numbering (entry gets the smallest number).
        let rpo: Vec<BlockId> = post.into_iter().rev().collect();
        for (i, &b) in rpo.iter().enumerate() {
            info.rpo_number[b.index()] = i;
        }

        // Predecessors restricted to reachable blocks.
        let mut preds: Vec<Vec<BlockId>> = vec![Vec::new(); n];
        for b in &rpo {
            for &s in &successors[b.index()] {
                if s.index() < n {
                    preds[s.index()].push(*b);
                }
            }
        }

        // Cooper–Harvey–Kennedy iterative dominator computation.
        info.idom[0] = Some(BlockId::new(0));
        let mut changed = true;
        while changed {
            changed = false;
            for &b in rpo.iter().skip(1) {
                let bi = b.index();
                let mut new_idom: Option<BlockId> = None;
                for &p in &preds[bi] {
                    if info.idom[p.index()].is_some() {
                        new_idom = match new_idom {
                            None => Some(p),
                            Some(cur) => Some(intersect(&info, p, cur)),
                        };
                    }
                }
                if let Some(nd) = new_idom {
                    if info.idom[bi] != Some(nd) {
                        info.idom[bi] = Some(nd);
                        changed = true;
                    }
                }
            }
        }
        info
    }

    /// Returns true when `a` dominates `b` (both must be reachable).
    pub(crate) fn dominates(&self, a: BlockId, b: BlockId) -> bool {
        let mut x = b;
        loop {
            if x == a {
                return true;
            }
            match self.idom[x.index()] {
                Some(p) if p != x => x = p,
                _ => return false,
            }
        }
    }

    /// Returns true when `a` strictly dominates `b`.
    pub(crate) fn strictly_dominates(&self, a: BlockId, b: BlockId) -> bool {
        a != b && self.dominates(a, b)
    }
}

fn intersect(info: &CfgInfo, a: BlockId, b: BlockId) -> BlockId {
    let (mut a, mut b) = (a, b);
    while a != b {
        while info.rpo_number[a.index()] > info.rpo_number[b.index()] {
            a = info.idom[a.index()].unwrap_or(a);
        }
        while info.rpo_number[b.index()] > info.rpo_number[a.index()] {
            b = info.idom[b.index()].unwrap_or(b);
        }
    }
    a
}

/// Position of a use inside a block. Block parameters occupy the "start"
/// slot; instruction `i` occupies `i + 1`; terminators occupy
/// `instructions.len() + 1`.
fn use_pos(f: &Function, block: usize) -> u32 {
    f.blocks[block].instructions.len() as u32 + 1
}

fn def_pos(value: &ValueKind) -> Option<(BlockId, u32)> {
    match value {
        ValueKind::BlockParam { block, .. } => Some((*block, 0)),
        ValueKind::Inst { block, index } => Some((*block, index + 1)),
        ValueKind::Param { .. } | ValueKind::Reserved => None,
    }
}

struct UseChecker<'a> {
    f: &'a Function,
    fname: &'a str,
    info: &'a CfgInfo,
    errors: &'a mut Vec<VerifyError>,
}

impl<'a> UseChecker<'a> {
    fn check(&mut self, v: ValueId, use_block: BlockId, pos: u32) {
        let data = match self.f.values.get(v.index()) {
            Some(d) => d,
            None => return, // Already reported as UnknownValue.
        };
        let def = match &data.kind {
            ValueKind::Param { .. } => return, // Dominates everything.
            ValueKind::Reserved => return,     // Already reported.
            ValueKind::BlockParam { .. } | ValueKind::Inst { .. } => {
                match def_pos(&data.kind) {
                    Some(d) => d,
                    None => return,
                }
            }
        };
        let (def_block, dpos) = def;
        let ub = use_block.index();
        let db = def_block.index();

        // Uses inside unreachable blocks are not checked (the block is dead
        // and may legally appear in any textual order, §34).
        if !self.info.reachable[ub] {
            return;
        }
        if !self.info.reachable[db] {
            self.errors.push(VerifyError::UseNotDominated {
                function: self.fname.to_string(),
                value: v.as_u32(),
                use_block: ub as u32,
                def_block: db as u32,
            });
            return;
        }
        let ok = if def_block == use_block {
            dpos < pos
        } else {
            self.info.strictly_dominates(def_block, use_block)
        };
        if !ok {
            self.errors.push(VerifyError::UseNotDominated {
                function: self.fname.to_string(),
                value: v.as_u32(),
                use_block: ub as u32,
                def_block: db as u32,
            });
        }
    }
}

pub(crate) fn check_cfg(
    module: &Module,
    f: &Function,
    name: &str,
    errors: &mut Vec<VerifyError>,
) {
    let info = CfgInfo::compute(f);

    for (bi, block) in f.blocks.iter().enumerate() {
        let from = BlockId::new(bi as u32);
        let term = match &block.terminator {
            Some(t) => t,
            None => {
                errors.push(VerifyError::MissingTerminator {
                    function: name.to_string(),
                    block: bi as u32,
                });
                continue;
            }
        };
        let check_edge = |to: BlockId,
                          args: &[ValueId],
                          errors: &mut Vec<VerifyError>| {
            let target = match f.blocks.get(to.index()) {
                Some(t) => t,
                None => {
                    errors.push(VerifyError::UnknownBlock {
                        function: name.to_string(),
                        block: to.as_u32(),
                    });
                    return;
                }
            };
            if target.params.len() != args.len() {
                errors.push(VerifyError::BlockArgCountMismatch {
                    function: name.to_string(),
                    from_block: from.as_u32(),
                    to_block: to.as_u32(),
                    expected: target.params.len(),
                    found: args.len(),
                });
            }
            for (i, (&arg, param)) in args.iter().zip(target.params.iter()).enumerate() {
                if let Some(actual) = f.values.get(arg.index()) {
                    if actual.ty != param.ty {
                        errors.push(VerifyError::BlockArgTypeMismatch {
                            function: name.to_string(),
                            from_block: from.as_u32(),
                            to_block: to.as_u32(),
                            index: i,
                            expected: module.type_name(param.ty),
                            found: module.type_name(actual.ty),
                        });
                    }
                }
            }
        };

        match term {
            Terminator::Jump { target, args } => check_edge(*target, args, errors),
            Terminator::Branch {
                condition,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                if let Some(c) = f.values.get(condition.index()) {
                    if c.ty != crate::id::TypeId::I1 {
                        errors.push(VerifyError::ConditionNotI1 {
                            function: name.to_string(),
                            found: module.type_name(c.ty),
                        });
                    }
                }
                check_edge(*then_block, then_args, errors);
                check_edge(*else_block, else_args, errors);
            }
            Terminator::Return { value } => {
                let is_void = module
                    .types()
                    .get(f.result)
                    .map(|d| d.is_void())
                    .unwrap_or(false);
                match value {
                    Some(v) => {
                        if is_void {
                            errors.push(VerifyError::ReturnMismatch {
                                function: name.to_string(),
                                expected: "void".to_string(),
                                found: format!(
                                    "a value of type `{}`",
                                    f.values
                                        .get(v.index())
                                        .map(|d| module.type_name(d.ty))
                                        .unwrap_or_else(|| "<unknown>".to_string())
                                ),
                            });
                        } else if let Some(d) = f.values.get(v.index()) {
                            if d.ty != f.result {
                                errors.push(VerifyError::ReturnMismatch {
                                    function: name.to_string(),
                                    expected: module.type_name(f.result),
                                    found: module.type_name(d.ty),
                                });
                            }
                        }
                    }
                    None => {
                        if !is_void {
                            errors.push(VerifyError::ReturnMismatch {
                                function: name.to_string(),
                                expected: module.type_name(f.result),
                                found: "void".to_string(),
                            });
                        }
                    }
                }
            }
            Terminator::Unreachable => {}
        }
    }

    // Dominance of uses (§12, §14).
    let mut checker = UseChecker {
        f,
        fname: name,
        info: &info,
        errors,
    };
    for (bi, block) in f.blocks.iter().enumerate() {
        let bid = BlockId::new(bi as u32);
        for (ii, inst) in block.instructions.iter().enumerate() {
            let pos = ii as u32 + 1;
            inst.kind
                .visit_values(&mut |v| checker.check(v, bid, pos));
        }
        let pos = use_pos(f, bi);
        if let Some(term) = &block.terminator {
            term.visit_values(&mut |v| checker.check(v, bid, pos));
        }
    }
}
