//! Go composite literals in the call graph.
//!
//! A literal (`T{}`, `pkg.T{}`) names a type, never a function: Go has no
//! constructors, and a getter often shares its type's name
//! (`func (d *DB) Message() Message`), so the name-based passes would bind
//! the literal to the getter. A literal resolves here instead, by package: a
//! bare `T` is the type declared in the literal's own package, and `pkg.T`
//! is the type declared in the directory the file imports as `pkg` (its
//! alias, or the package name the imported files declare). A dot import
//! brings its package's types in bare.

use crate::calls::graph::{CallKindCompat, CallTarget, CallableMeta, Confidence, Qn};
use crate::deps::resolver::Lang;
use crate::deps::DepGraph;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Whether an edge of `kind`, written in `lang`, is a Go literal, which
/// resolves through [`resolve_literal`] and is no call of a function.
pub(crate) fn is_literal(lang: Option<Lang>, kind: CallKindCompat) -> bool {
  lang == Some(Lang::Go) && matches!(kind, CallKindCompat::Construct)
}

/// The type that a literal naming `name`, qualified by `receiver`, in the
/// absolute path `file` and the function `source`, constructs. A directory
/// can hold two packages (`models` and its external test package
/// `models_test`): a bare name is looked up in the source's own package,
/// and an import never names a `_test` package. One type is `Exact`. None
/// (a type from outside the project) leaves the edge unresolved by name;
/// several (one declaration per build tag) leave it unresolved with them as
/// candidates.
pub(crate) fn resolve_literal(
  root: &Path,
  deps: &DepGraph,
  file: &Path,
  source: &Qn,
  receiver: Option<&str>,
  name: &str,
  type_by_name: &HashMap<String, Vec<Qn>>,
  callable_meta: &HashMap<Qn, CallableMeta>,
) -> (CallTarget, Confidence, Vec<Qn>) {
  let dir_of = |qn: &Qn| -> Option<PathBuf> {
    let path = root.join(qn.file().replace('/', std::path::MAIN_SEPARATOR_STR));
    path.parent().map(Path::to_path_buf)
  };
  let types = type_by_name.get(name).map_or(&[][..], Vec::as_slice);
  let go = types
    .iter()
    .filter(|qn| crate::calls::resolve::qn_lang(qn) == Some(Lang::Go));
  let imports = deps.forward.get(file).map_or(&[][..], Vec::as_slice);
  let own_package = source.0.split("::").nth(1);
  let importable = |qn: &Qn| !package_of(qn).is_some_and(|p| p.ends_with("_test"));
  let mut found: Vec<Qn> = match receiver {
    // A dot import (`import . "shop/a"`) brings its package's names in bare.
    None => go
      .filter(|qn| {
        let dir = dir_of(qn);
        (dir.as_deref() == file.parent() && package_of(qn) == own_package)
          || (importable(qn)
            && imports
              .iter()
              .any(|e| e.local_name.as_deref() == Some(".") && e.target.parent() == dir.as_deref()))
      })
      .cloned()
      .collect(),
    Some(pkg) => go
      .filter(|qn| {
        let dir = dir_of(qn);
        importable(qn)
          && imports.iter().any(|e| {
            let binds = match &e.local_name {
              Some(alias) => alias == pkg,
              None => package_of(qn) == Some(pkg),
            };
            binds && e.target.parent() == dir.as_deref()
          })
      })
      .cloned()
      .collect(),
  };
  found.sort();
  found.dedup();
  match found.len() {
    // A method whose receiver type is declared in another file sits at
    // package level, so `(d *DB) Message()` can share its qn with the type
    // `Message` beside it. An edge to that qn would read as a call of the
    // method everywhere a qn is looked up, so the literal keeps the type
    // as its one candidate instead.
    1 if callable_meta.contains_key(&found[0]) => (
      CallTarget::Bare(name.to_string()),
      Confidence::Ambiguous,
      found,
    ),
    1 => (
      CallTarget::Resolved(found.remove(0)),
      Confidence::Exact,
      Vec::new(),
    ),
    0 => (
      CallTarget::Bare(name.to_string()),
      Confidence::Ambiguous,
      Vec::new(),
    ),
    _ => (
      CallTarget::Bare(name.to_string()),
      Confidence::Ambiguous,
      found,
    ),
  }
}

/// The package a Go type's qn declares it in (`a/a.go::a::Config` → `a`).
fn package_of(qn: &Qn) -> Option<&str> {
  let mut segments = qn.0.split("::");
  segments.next()?;
  let package = segments.next()?;
  segments.next().map(|_| package)
}
