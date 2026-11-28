//! Opening a YouTube video in the television's own YouTube app.
//!
//! This app cannot play YouTube itself (the stream URLs are not public), so a trailer Seerr lists
//! leaves for the YouTube app through the system's application manager, the way a phone's "cast"
//! button does. The app id and the `contentTarget` launch parameter are the community-documented
//! contract of the webOS YouTube app (webosbrew's command cheatsheet); the reply is graded, so a
//! set without that app answers `false` rather than doing nothing silently.
use serde_json::json;

/// The webOS YouTube app.
const APP_ID: &str = "youtube.leanback.v4";

/// A YouTube video id is letters, digits, `-` and `_`; the id goes into a URL inside a launch
/// request, so anything else is refused.
fn is_video_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The launch request for `video`, or `None` when it is not a YouTube video id.
fn request(video: &str) -> Option<String> {
    is_video_id(video).then(|| {
        json!({ "id": APP_ID, "params": { "contentTarget": format!("https://www.youtube.com/watch?v={video}") } }).to_string()
    })
}

/// Open `video` in the YouTube app; `true` once the system accepted the launch. Blocks for the
/// answer, so call it off the frame thread.
pub fn watch(video: &str) -> bool {
    match request(video) {
        Some(payload) => (super::port().launch_app)(&payload),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_video_id_becomes_one_launch_request_for_the_youtube_app() {
        let sent: serde_json::Value = serde_json::from_str(&request("m8e-h2R9pZU").unwrap()).unwrap();
        assert_eq!(sent["id"], "youtube.leanback.v4");
        assert_eq!(sent["params"]["contentTarget"], "https://www.youtube.com/watch?v=m8e-h2R9pZU");
    }

    #[test]
    fn anything_that_is_not_a_video_id_launches_nothing() {
        for bad in ["", "a b", "x&y=1", "a\"b", "../x", "ünï"] {
            assert!(request(bad).is_none(), "{bad:?}");
            assert!(!watch(bad));
        }
    }
}
