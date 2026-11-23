//! **Radarr's and Sonarr's HTTP API (v3)**, as far as taking a deleted title out of them needs it.
//! The contract is the apps' own source, read 2026-11-22:
//!
//! * every call carries `X-Api-Key`; a wrong key answers `401`;
//! * `GET /api/v3/movie?tmdbId=` and `GET /api/v3/series?tvdbId=` answer an array, empty when the
//!   app does not hold the title; each entry has its own `id`;
//! * `DELETE /api/v3/movie/{id}?deleteFiles=true&addImportExclusion=true` and
//!   `DELETE /api/v3/series/{id}?deleteFiles=true&addImportListExclusion=true` remove the title,
//!   its files, and add it to the import-list exclusions, which only a list sync reads: a request or
//!   a manual add of the same title later still works. Nothing is blocklisted;
//! * a title the app does not hold gets only the exclusion: `POST /api/v3/exclusions` (Radarr,
//!   `{tmdbId, movieTitle, movieYear}`) or `POST /api/v3/importlistexclusion` (Sonarr,
//!   `{tvdbId, title}`), which answers `400` "already" when it exists.
//!
//! Pure but for [`CurlTransport`]: everything else takes a [`Transport`], so the tests drive the flows
//! against a fake. The key only ever goes in a header; no value here holds it and nothing logs a URL.

pub use super::bazarr::TransportError;
use super::session::{ArrKind, ArrLink};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Get,
    Post,
    Delete,
}

/// One request: `path` starts with `/api/v3/` and carries its own query; `json` is a POST's body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub verb: Verb,
    pub path: String,
    pub json: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

pub trait Transport {
    fn send(&self, link: &ArrLink, request: &Request) -> Result<Reply, TransportError>;
}

/// The real transport: libcurl through the `net` layer, verified against the TV's trust store and,
/// when that cannot verify the chain, the bundled public roots (as Bazarr's).
pub struct CurlTransport;

impl Transport for CurlTransport {
    fn send(&self, link: &ArrLink, request: &Request) -> Result<Reply, TransportError> {
        use plx_net::net::{self, RequestError, Timeouts, Tls};
        let url = format!("{}{}", link.url.trim_end_matches('/'), request.path);
        let mut headers = vec![format!("X-Api-Key: {}", link.api_key), "Accept: application/json".to_string()];
        let body = request.json.as_ref().map(|json| {
            headers.push("Content-Type: application/json".to_string());
            json.clone().into_bytes()
        });
        // Deleting files runs on the app's own disks before it answers.
        let timeouts = match request.verb {
            Verb::Delete => Timeouts { connect_s: 8, total_s: 150, total_ms: 0, low_speed_bps: 0, low_speed_s: 0 },
            _ => net::API,
        };
        let verb = match request.verb {
            Verb::Get => "GET",
            Verb::Post => "POST",
            Verb::Delete => "DELETE",
        };
        let failure = match net::request_evidence(&url, &headers, verb, body.as_deref(), timeouts, false, Some(MAX_BODY), None) {
            Ok(resp) => return Ok(Reply { status: resp.status, body: resp.body }),
            Err(failure) => failure,
        };
        if failure.untrusted_chain().is_some() {
            let bundle = plx_base::paths::app_dir().join(net::keypin::ROOTS_FILE);
            if let Some(path) = bundle.to_str().filter(|_| bundle.is_file()) {
                if let Some(resp) = net::request_tls(&url, &headers, verb, body.as_deref(), timeouts, false, Some(MAX_BODY), Tls::CaBundle(path)) {
                    return Ok(Reply { status: resp.status, body: resp.body });
                }
            }
        }
        match failure.cause {
            RequestError::TimedOut => Err(TransportError::TimedOut),
            RequestError::Transport => Err(TransportError::Unreachable),
        }
    }
}

/// A library lookup answers one entry; the biggest body read is far above that.
const MAX_BODY: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrError {
    /// The app refused the key (401/403): trying again changes nothing.
    Refused,
    /// No answer, or an error: a later try may work.
    Failed,
}

pub struct Arr<'a, T: Transport> {
    link: &'a ArrLink,
    http: &'a T,
}

impl<'a, T: Transport> Arr<'a, T> {
    pub fn new(link: &'a ArrLink, http: &'a T) -> Self {
        Self { link, http }
    }

    fn send(&self, verb: Verb, path: String, json: Option<Value>) -> Result<Reply, ArrError> {
        let reply = self.http.send(self.link, &Request { verb, path, json: json.map(|v| v.to_string()) })
            .map_err(|_| ArrError::Failed)?;
        match reply.status {
            401 | 403 => Err(ArrError::Refused),
            _ => Ok(reply),
        }
    }

    /// Take the title out: delete it and its files when the app holds it, and keep it from coming
    /// back through a list sync. `id` is the TMDB id of a movie or the TVDB id of a show.
    pub fn remove(&self, kind: ArrKind, id: i64, title: &str, year: i64) -> Result<(), ArrError> {
        let (lookup, held, delete, flag) = match kind {
            ArrKind::Radarr => (format!("/api/v3/movie?tmdbId={id}"), "/api/v3/movie", "addImportExclusion", "/api/v3/exclusions"),
            ArrKind::Sonarr => (format!("/api/v3/series?tvdbId={id}"), "/api/v3/series", "addImportListExclusion", "/api/v3/importlistexclusion"),
        };
        let reply = self.send(Verb::Get, lookup, None)?;
        if !(200..300).contains(&reply.status) {
            return Err(ArrError::Failed);
        }
        let own = serde_json::from_slice::<Value>(&reply.body).ok()
            .and_then(|v| v.as_array()?.first()?.get("id")?.as_i64());
        if let Some(own) = own {
            let reply = self.send(Verb::Delete, format!("{held}/{own}?deleteFiles=true&{delete}=true"), None)?;
            match reply.status {
                200..=299 => return Ok(()),
                404 => {}
                _ => return Err(ArrError::Failed),
            }
        }
        let body = match kind {
            ArrKind::Radarr => json!({ "tmdbId": id, "movieTitle": title, "movieYear": year }),
            ArrKind::Sonarr => json!({ "tvdbId": id, "title": title }),
        };
        let reply = self.send(Verb::Post, flag.into(), Some(body))?;
        match reply.status {
            200..=299 => Ok(()),
            400 if String::from_utf8_lossy(&reply.body).to_ascii_lowercase().contains("already") => Ok(()),
            _ => Err(ArrError::Failed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Answers by exact `VERB path` and records what was sent.
    struct Fake {
        routes: Vec<(&'static str, u16, &'static str)>,
        seen: RefCell<Vec<String>>,
    }

    impl Transport for Fake {
        fn send(&self, link: &ArrLink, request: &Request) -> Result<Reply, TransportError> {
            assert!(link.is_complete());
            let verb = match request.verb { Verb::Get => "GET", Verb::Post => "POST", Verb::Delete => "DELETE" };
            let line = format!("{verb} {}", request.path);
            self.seen.borrow_mut().push(match &request.json {
                Some(json) => format!("{line} {json}"),
                None => line.clone(),
            });
            let (_, status, body) = self.routes.iter().find(|(r, ..)| *r == line).ok_or(TransportError::Unreachable)?;
            Ok(Reply { status: *status, body: body.as_bytes().to_vec() })
        }
    }

    fn fake(routes: Vec<(&'static str, u16, &'static str)>) -> Fake {
        Fake { routes, seen: RefCell::new(vec![]) }
    }

    /// The JSON body of the one POST sent.
    fn posted(http: &Fake) -> Value {
        let seen = http.seen.borrow();
        let line = seen.iter().find(|l| l.starts_with("POST ")).expect("a POST was sent");
        serde_json::from_str(&line[line.find(' ').and_then(|i| line[i + 1..].find(' ').map(|j| i + 1 + j)).unwrap() + 1..]).unwrap()
    }

    fn link() -> ArrLink {
        ArrLink { url: "https://arr.example.net".into(), api_key: "k".into() }
    }

    #[test]
    fn a_held_movie_is_deleted_with_its_files_and_excluded_and_nothing_else_is_sent() {
        let http = fake(vec![
            ("GET /api/v3/movie?tmdbId=603", 200, r#"[{"id":7,"title":"The Matrix"}]"#),
            ("DELETE /api/v3/movie/7?deleteFiles=true&addImportExclusion=true", 200, ""),
        ]);
        assert_eq!(Arr::new(&link(), &http).remove(ArrKind::Radarr, 603, "The Matrix", 1999), Ok(()));
        assert_eq!(http.seen.borrow().len(), 2, "no blocklist, no second delete");
    }

    #[test]
    fn a_held_show_is_deleted_by_its_series_id() {
        let http = fake(vec![
            ("GET /api/v3/series?tvdbId=81189", 200, r#"[{"id":3}]"#),
            ("DELETE /api/v3/series/3?deleteFiles=true&addImportListExclusion=true", 200, ""),
        ]);
        assert_eq!(Arr::new(&link(), &http).remove(ArrKind::Sonarr, 81189, "Breaking Bad", 2008), Ok(()));
    }

    #[test]
    fn a_title_the_app_does_not_hold_or_just_lost_gets_only_the_exclusion() {
        let radarr = fake(vec![
            ("GET /api/v3/movie?tmdbId=603", 200, "[]"),
            ("POST /api/v3/exclusions", 201, "{}"),
        ]);
        assert_eq!(Arr::new(&link(), &radarr).remove(ArrKind::Radarr, 603, "The Matrix", 1999), Ok(()));
        assert_eq!(posted(&radarr), json!({"tmdbId": 603, "movieTitle": "The Matrix", "movieYear": 1999}));
        let sonarr = fake(vec![
            ("GET /api/v3/series?tvdbId=5", 200, r#"[{"id":3}]"#),
            ("DELETE /api/v3/series/3?deleteFiles=true&addImportListExclusion=true", 404, ""),
            ("POST /api/v3/importlistexclusion", 400, r#"[{"errorMessage":"This exclusion has already been added."}]"#),
        ]);
        assert_eq!(Arr::new(&link(), &sonarr).remove(ArrKind::Sonarr, 5, "Show", 2000), Ok(()));
        assert_eq!(posted(&sonarr), json!({"tvdbId": 5, "title": "Show"}));
    }

    #[test]
    fn a_refused_key_is_refused_and_anything_else_that_goes_wrong_may_be_tried_again() {
        let http = fake(vec![("GET /api/v3/movie?tmdbId=1", 401, "")]);
        assert_eq!(Arr::new(&link(), &http).remove(ArrKind::Radarr, 1, "x", 1), Err(ArrError::Refused));
        let down = fake(vec![]);
        assert_eq!(Arr::new(&link(), &down).remove(ArrKind::Radarr, 1, "x", 1), Err(ArrError::Failed));
        let broken = fake(vec![
            ("GET /api/v3/movie?tmdbId=1", 200, r#"[{"id":2}]"#),
            ("DELETE /api/v3/movie/2?deleteFiles=true&addImportExclusion=true", 500, ""),
        ]);
        assert_eq!(Arr::new(&link(), &broken).remove(ArrKind::Radarr, 1, "x", 1), Err(ArrError::Failed));
        let other = fake(vec![
            ("GET /api/v3/movie?tmdbId=1", 200, "[]"),
            ("POST /api/v3/exclusions", 400, "[]"),
        ]);
        assert_eq!(Arr::new(&link(), &other).remove(ArrKind::Radarr, 1, "x", 1), Err(ArrError::Failed), "only an 'already' 400 is done");
    }
}
