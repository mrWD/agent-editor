//! Pictures in messages.
//!
//! A picture the user puts into a message is kept on disk next to the rest of
//! the data, named in the conversation, and sent to the agent as an image
//! block. Only agents that say they take images get one; the others are not
//! sent a picture they would not see.

use crate::tasks::Manager;
use anyhow::{Context, Result, bail};
use base64::Engine;
use serde_json::{Value, json};
use std::path::PathBuf;

/// How many pictures one message can carry, and how large each can be.
pub const MAX_PICTURES: usize = 5;
pub const MAX_BYTES: usize = 5 * 1024 * 1024;

/// The kinds of picture agents take, with the ending each is kept under.
const KINDS: [(&str, &str); 4] =
    [("image/png", "png"), ("image/jpeg", "jpg"), ("image/gif", "gif"), ("image/webp", "webp")];

/// A picture as the interface sends it.
#[derive(Debug, Clone)]
pub struct Picture {
    pub name: String,
    pub mime_type: String,
    /// The bytes, in base64.
    pub data: String,
}

impl Picture {
    /// Reads the pictures of a `tasks.prompt` command. Nothing given means no pictures.
    pub fn list_from(params: &Value) -> Result<Vec<Picture>> {
        let Some(list) = params.get("images").and_then(Value::as_array) else { return Ok(vec![]) };
        list.iter()
            .map(|one| {
                let text = |key: &str| one.get(key).and_then(Value::as_str).map(str::to_string);
                Ok(Picture {
                    name: text("name").unwrap_or_else(|| "picture".into()),
                    mime_type: text("mimeType").context("A picture needs its kind (mimeType)")?,
                    data: text("data").context("A picture needs its bytes (data)")?,
                })
            })
            .collect()
    }
}

/// A picture checked and ready: what goes to the agent and what is kept.
pub(crate) struct Kept {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub data: String,
    pub size: usize,
}

impl Kept {
    /// How the conversation names the picture. The bytes are not in it.
    pub fn named(&self) -> Value {
        json!({ "id": self.id, "name": self.name, "mimeType": self.mime_type, "size": self.size })
    }

    /// The block an agent gets.
    pub fn block(&self) -> Value {
        json!({ "type": "image", "mimeType": self.mime_type, "data": self.data })
    }
}

fn ending_of(mime_type: &str) -> Option<&'static str> {
    KINDS.iter().find(|(kind, _)| *kind == mime_type).map(|(_, ending)| *ending)
}

/// The name a picture is shown under: its own, without any folders, and not too long.
fn plain_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    let short: String = last.chars().take(80).collect();
    if short.is_empty() { "picture".into() } else { short }
}

impl Manager {
    fn pictures_dir(&self, task_id: &str) -> PathBuf {
        self.data_dir().join("pictures").join(task_id)
    }

    /// Checks the pictures of a message and writes them to disk.
    pub(crate) fn keep_pictures(&self, task_id: &str, pictures: &[Picture]) -> Result<Vec<Kept>> {
        if pictures.len() > MAX_PICTURES {
            bail!("A message takes {MAX_PICTURES} pictures at most.");
        }
        let mut kept = Vec::new();
        for picture in pictures {
            let name = plain_name(&picture.name);
            let Some(ending) = ending_of(&picture.mime_type) else {
                bail!("{name} is not a picture an agent can take. PNG, JPEG, GIF and WebP are.");
            };
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(picture.data.trim())
                .with_context(|| format!("{name} did not arrive whole. Add it again."))?;
            if bytes.is_empty() {
                bail!("{name} is empty.");
            }
            if bytes.len() > MAX_BYTES {
                bail!("{name} is larger than {} MB.", MAX_BYTES / 1024 / 1024);
            }
            let id = format!("{}.{ending}", uuid::Uuid::new_v4());
            let dir = self.pictures_dir(task_id);
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join(&id), &bytes)?;
            kept.push(Kept {
                id,
                name,
                mime_type: picture.mime_type.clone(),
                data: picture.data.trim().to_string(),
                size: bytes.len(),
            });
        }
        Ok(kept)
    }

    /// Where a kept picture is, and its kind. `None` for anything that is not the name of a kept picture.
    pub fn picture(&self, task_id: &str, id: &str) -> Option<(PathBuf, &'static str)> {
        let plain =
            |text: &str| !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.');
        if !plain(task_id) || !plain(id) || id.starts_with('.') || id.contains("..") {
            return None;
        }
        let (_, ending) = id.rsplit_once('.')?;
        let (kind, _) = KINDS.iter().find(|(_, known)| *known == ending)?;
        let path = self.pictures_dir(task_id).join(id);
        path.is_file().then_some((path, *kind))
    }

    /// Forgets the pictures of a task that is gone for good.
    pub(crate) fn drop_pictures(&self, task_id: &str) {
        let _ = std::fs::remove_dir_all(self.pictures_dir(task_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_shown_without_folders_and_a_kind_has_its_ending() {
        assert_eq!(plain_name("C:\\shots\\login errors.png"), "login errors.png");
        assert_eq!(plain_name("/tmp/a/b.png"), "b.png");
        assert_eq!(plain_name("  "), "picture");
        assert_eq!(plain_name(&"x".repeat(200)).chars().count(), 80);
        assert_eq!(ending_of("image/jpeg"), Some("jpg"));
        assert_eq!(ending_of("image/svg+xml"), None);
    }

    #[test]
    fn the_pictures_of_a_command_are_read() {
        let params = json!({ "images": [{ "name": "a.png", "mimeType": "image/png", "data": "AA==" }] });
        let list = Picture::list_from(&params).unwrap();
        assert_eq!((list.len(), list[0].name.as_str(), list[0].mime_type.as_str()), (1, "a.png", "image/png"));
        assert!(Picture::list_from(&json!({})).unwrap().is_empty());
        assert!(Picture::list_from(&json!({ "images": [{ "name": "a.png" }] })).is_err());
    }
}
