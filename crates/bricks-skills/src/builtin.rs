//! Skills built into Bricks: standard `SKILL.md` documents compiled into the
//! binary, read by the same parser as any other. They describe procedures;
//! none runs, schedules or commits anything by being activated. They have no
//! files and nothing is extracted to the user's folders.

use crate::registry::BuiltinSkill;

pub const BUILTIN: &[BuiltinSkill] = &[
    BuiltinSkill {
        name: "commit",
        text: include_str!("../builtin/commit/SKILL.md"),
    },
    BuiltinSkill {
        name: "debug",
        text: include_str!("../builtin/debug/SKILL.md"),
    },
    BuiltinSkill {
        name: "remember",
        text: include_str!("../builtin/remember/SKILL.md"),
    },
    BuiltinSkill {
        name: "simplify",
        text: include_str!("../builtin/simplify/SKILL.md"),
    },
    BuiltinSkill {
        name: "stuck",
        text: include_str!("../builtin/stuck/SKILL.md"),
    },
    BuiltinSkill {
        name: "verify",
        text: include_str!("../builtin/verify/SKILL.md"),
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::parse_document;

    #[test]
    fn every_builtin_is_a_valid_standard_document() {
        for b in BUILTIN {
            let d =
                parse_document(b.text, Some(b.name)).unwrap_or_else(|e| panic!("{}: {e}", b.name));
            assert_eq!(d.name, b.name);
            assert!(d.is_compatible(), "{}", b.name);
            assert!(d.notes.is_empty(), "{}: {:?}", b.name, d.notes);
            assert!(!d.body.contains("$ARGUMENTS"), "{}: no template", b.name);
            assert!(crate::security::scan(&d.body).is_empty(), "{}", b.name);
        }
    }

    #[test]
    fn builtins_name_only_tools_bricks_has() {
        // The procedures rely on the standard tool set; nothing like
        // CronCreate or a memory tool is assumed.
        for b in BUILTIN {
            for absent in ["CronCreate", "CronList", "MemoryWrite", "$ARGUMENTS"] {
                assert!(!b.text.contains(absent), "{}: {absent}", b.name);
            }
        }
    }
}

#[cfg(test)]
mod repository_examples {
    use crate::document::parse_document;
    use std::path::Path;

    /// The example skills of the repository parse with the same parser,
    /// and their resources stay inside their directory.
    #[test]
    fn examples_are_valid_standard_skills() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/skills");
        for name in ["hello-skill", "release-notes"] {
            let dir = root.join(name);
            let text = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
            let d = parse_document(&text, Some(name)).unwrap();
            assert!(
                d.is_compatible() && d.notes.is_empty(),
                "{name}: {:?}",
                d.notes
            );
            assert!(crate::security::scan(&d.body).is_empty(), "{name}");
        }
        let (list, _) = crate::resources::list(&root.join("release-notes"));
        assert_eq!(list[0].path, "references/format.md");
        assert!(
            crate::resources::resolve(&root.join("release-notes"), "references/format.md").is_ok()
        );
    }
}
