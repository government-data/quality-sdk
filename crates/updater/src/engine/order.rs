//! The order a platform version's modules are applied in (decision 0003): a
//! module after the modules that provide the capabilities it requires. The
//! engine names no module: the order comes from what each one declares.

use std::collections::BTreeSet;

use super::OfferedPackage;

/// The packages in the order they are applied. Among those that can go
/// next, by name, so the order is always the same. A requirement that no
/// module of the version provides does not order anything (whether it is
/// met is checked when a module is added). Two packages for one module, or
/// modules that require each other, are refused.
pub(super) fn apply_order(packages: &[OfferedPackage]) -> Result<Vec<OfferedPackage>, String> {
    let mut left: Vec<&OfferedPackage> = packages.iter().collect();
    left.sort_by(|a, b| a.module.cmp(&b.module));
    if let Some(w) = left.windows(2).find(|w| w[0].module == w[1].module) {
        return Err(format!("two packages for the module {}", w[0].module));
    }
    let mut ordered = Vec::with_capacity(left.len());
    while !left.is_empty() {
        let provided_by_left = |capability: &str, other: &str| {
            left.iter()
                .any(|p| p.module != other && p.provides.iter().any(|c| c == capability))
        };
        let Some(i) = left
            .iter()
            .position(|p| p.requires.iter().all(|c| !provided_by_left(c, &p.module)))
        else {
            let names: BTreeSet<_> = left.iter().map(|p| p.module.as_str()).collect();
            let names: Vec<_> = names.into_iter().collect();
            return Err(format!(
                "the modules {} require each other",
                names.join(", ")
            ));
        };
        ordered.push(left.remove(i).clone());
    }
    Ok(ordered)
}
