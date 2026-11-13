//! Pictures in messages: kept, named in the conversation, sent to agents that take them.

mod common;

use ae_core::pictures::{MAX_BYTES, MAX_PICTURES, Picture};
use common::*;
use serde_json::Value;

/// The smallest PNG there is: one transparent pixel, 70 bytes.
const PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

fn png(name: &str) -> Picture {
    Picture { name: name.into(), mime_type: "image/png".into(), data: PIXEL.into() }
}

#[tokio::test]
async fn a_picture_goes_to_the_agent_and_is_kept_for_the_conversation() {
    let sandbox = Sandbox::new("pictures");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    // The agent said at the start that it takes pictures; the interface is told.
    assert_eq!(manager.list().unwrap()[0].config.as_ref().unwrap()["images"], true);

    manager
        .prompt_with_pictures(&id, "What is wrong here?".into(), vec![png("/shots/login errors.png"), png("b.png")])
        .await
        .unwrap();
    let turn = until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(answer_text(&turn), "I see 2 pictures: image/png, 70 bytes, image/png, 70 bytes. Nothing was changed.");

    // The conversation names the pictures; their bytes are on disk, not in the history.
    let said = turn.iter().find(|e| is(e, "user_message")).unwrap();
    let named = said["images"].as_array().unwrap();
    assert_eq!(named.len(), 2);
    assert_eq!((named[0]["name"].as_str(), named[0]["size"].as_u64()), (Some("login errors.png"), Some(70)));
    assert!(named[0].get("data").is_none());
    let picture_id = named[0]["id"].as_str().unwrap();
    let (path, kind) = manager.picture(&id, picture_id).unwrap();
    assert_eq!(kind, "image/png");
    assert_eq!(std::fs::read(&path).unwrap().len(), 70);
    assert!(path.starts_with(manager.data_dir()));

    // Only the name of a kept picture is a way to one.
    assert!(manager.picture(&id, "../../ae.db").is_none());
    assert!(manager.picture("..", picture_id).is_none());
    assert!(manager.picture(&id, "nothing.png").is_none());
    assert!(manager.picture(&id, &picture_id.replace(".png", ".exe")).is_none());

    // A picture needs no words with it.
    manager.prompt_with_pictures(&id, String::new(), vec![png("alone.png")]).await.unwrap();
    let turn = until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert!(answer_text(&turn).starts_with("I see one picture"));
    // Without a picture an empty message is still refused.
    assert!(manager.prompt_with_pictures(&id, "  ".into(), vec![]).await.is_err());

    // A new agent is told that pictures were shown: it cannot be shown them again in words.
    manager.stop_agent(&id).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "asleep")).await;
    manager.prompt(&id, "And now?".into()).await.unwrap();
    let turn = until(&mut events, &id, |e| state_is(e, "idle")).await;
    let thought: String = turn
        .iter()
        .filter(|e| e["update"]["sessionUpdate"] == "agent_thought_chunk")
        .filter_map(|e| e["update"]["content"]["text"].as_str())
        .collect();
    assert!(thought.contains("User: What is wrong here? (with 2 pictures the user showed)"), "{thought}");
    assert!(thought.contains("User: (with a picture the user showed)"), "{thought}");

    // Archived, the task keeps nothing on disk.
    manager.archive(&id).await.unwrap();
    assert!(manager.picture(&id, picture_id).is_none());
}

#[tokio::test]
async fn pictures_that_cannot_be_sent_are_refused_with_the_reason() {
    let sandbox = Sandbox::new("pictures-refused");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    let refused = |pictures: Vec<Picture>| {
        let (manager, id) = (manager.clone(), id.clone());
        async move { manager.prompt_with_pictures(&id, "Look".into(), pictures).await.unwrap_err().to_string() }
    };

    let many = (0..=MAX_PICTURES).map(|n| png(&format!("{n}.png"))).collect();
    assert!(refused(many).await.contains("5 pictures at most"));
    let other = Picture { name: "page.svg".into(), mime_type: "image/svg+xml".into(), data: PIXEL.into() };
    assert!(refused(vec![other]).await.contains("page.svg is not a picture an agent can take"));
    let broken = Picture { name: "cut.png".into(), mime_type: "image/png".into(), data: "not base64!".into() };
    assert!(refused(vec![broken]).await.contains("cut.png did not arrive whole"));
    let large =
        Picture { name: "large.png".into(), mime_type: "image/png".into(), data: "A".repeat((MAX_BYTES + 3) / 3 * 4) };
    assert!(refused(vec![large]).await.contains("large.png is larger than 5 MB"));
    // Nothing of a refused message is kept or said.
    assert!(!manager.data_dir().join("pictures").join(&id).exists());
    assert!(manager.history(&id).unwrap().iter().filter(|e: &&Value| is(e, "user_message")).count() == 1);
    // The task is still ready for the next message.
    manager.prompt(&id, "What does it do?".into()).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;

    // An agent that takes no pictures is not sent one, and says so through the editor.
    let old = manager.create(&project.id, "mock-modes", "What does the limiter do?").await.unwrap();
    until(&mut events, &old.row.id, |e| state_is(e, "idle")).await;
    assert_eq!(
        manager.list().unwrap().iter().find(|t| t.row.id == old.row.id).unwrap().config.as_ref().unwrap()["images"],
        false
    );
    let error = manager.prompt_with_pictures(&old.row.id, "Look".into(), vec![png("a.png")]).await.unwrap_err();
    assert!(error.to_string().contains("does not take pictures"), "{error}");
    manager.stop_all().await;
}
