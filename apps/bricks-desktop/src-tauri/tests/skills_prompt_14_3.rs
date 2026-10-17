//! Sprint 14.3, validation: a message sent from the composer with skills chosen
//! in the `+` menu. The JSON is the one the frontend sends (`session_command`
//! forwards it unchanged, see `src/lib/features/composer/Composer.svelte` `send`
//! and `src/lib/ipc/engine.ts` `Command`/`Prompt`): it must read back as the
//! engine's `Command::Submit`, with the text, the attachments in order and the
//! skill names as chosen. Activation itself (refusal of an unusable skill, trust
//! of project skills) is the engine's and is covered by its own tests.

use cersei_agent::control::{Command, Prompt, PromptBlock};
use serde_json::json;

fn submit_of(command: Command) -> Prompt {
    match command {
        Command::Submit { prompt } => prompt,
        other => panic!("expected a submit, got {other:?}"),
    }
}

#[test]
fn text_attachments_and_skills_read_back_as_the_engine_submit() {
    let wire = json!({
        "type": "submit",
        "prompt": {
            "blocks": [
                { "kind": "text", "text": "Relis ce dossier et résume-le" },
                { "kind": "file", "path": "/Users/moi/Projets/notes de réunion.md" },
                { "kind": "folder", "path": "/Users/moi/Projets/docs" },
                { "kind": "image", "path": "/Users/moi/capture.png" }
            ],
            "skills": ["revue", "Synthèse fr"]
        }
    });
    let command: Command = serde_json::from_value(wire).expect("the frontend's submit is a Command");
    let prompt = submit_of(command);

    assert_eq!(
        prompt.blocks,
        vec![
            PromptBlock::Text {
                text: "Relis ce dossier et résume-le".into()
            },
            PromptBlock::File {
                path: "/Users/moi/Projets/notes de réunion.md".into()
            },
            PromptBlock::Folder {
                path: "/Users/moi/Projets/docs".into()
            },
            PromptBlock::Image {
                path: "/Users/moi/capture.png".into()
            },
        ],
        "text and attachments keep their order and their paths as typed"
    );
    assert_eq!(prompt.skills, vec!["revue".to_string(), "Synthèse fr".to_string()]);
}

#[test]
fn a_message_without_skills_sends_no_skill_field() {
    // The composer sends `{ blocks }` alone when nothing is chosen.
    let wire = json!({ "type": "submit", "prompt": { "blocks": [{ "kind": "text", "text": "bonjour" }] } });
    let prompt = submit_of(serde_json::from_value(wire).unwrap());
    assert!(prompt.skills.is_empty());
    assert_eq!(prompt.blocks, vec![PromptBlock::Text { text: "bonjour".into() }]);

    // And the engine's own serialisation leaves the field out, as the frontend expects.
    let back = serde_json::to_value(Command::Submit { prompt }).unwrap();
    assert!(back["prompt"].get("skills").is_none(), "{back}");
}

#[test]
fn a_skill_only_message_keeps_its_names_and_no_text() {
    // A selection with an empty text is still a message: its blocks may be empty.
    let wire = json!({ "type": "submit", "prompt": { "blocks": [], "skills": ["revue"] } });
    let prompt = submit_of(serde_json::from_value(wire).unwrap());
    assert!(prompt.blocks.is_empty());
    assert_eq!(prompt.skills, vec!["revue".to_string()]);
}

#[test]
fn an_unknown_block_kind_is_refused_by_the_engine_not_guessed() {
    // The frontend only builds text, file, folder and image blocks.
    let wire = json!({ "type": "submit", "prompt": { "blocks": [{ "kind": "video", "path": "/x.mov" }] } });
    assert!(serde_json::from_value::<Command>(wire).is_err());
}
