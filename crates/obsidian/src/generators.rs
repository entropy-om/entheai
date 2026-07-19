//! Pure per-note generators. Each is per-source conditional: a missing source
//! contributes nothing (never an error).

use crate::render::{RenderOutput, RepoContext};

pub fn docs_mirror(_ctx: &RepoContext, _out: &mut RenderOutput) {}
pub fn architecture(_ctx: &RepoContext, _out: &mut RenderOutput) {}
pub fn sessions(_ctx: &RepoContext, _out: &mut RenderOutput) {}
pub fn section_indexes(_ctx: &RepoContext, _out: &mut RenderOutput) {}
pub fn home_moc(_ctx: &RepoContext, _out: &mut RenderOutput) {}
