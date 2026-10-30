//! **Bazarr's HTTP API**, as far as "Wrong subtitles?" needs it: check the connection, find the
//! viewer's item in Bazarr, ask its providers for subtitles, and download the one the viewer
//! picks. The contract is Bazarr's own source (`bazarr/api/…`, read 2026-10-30):
//!
//! * every call carries `X-API-KEY`; a wrong or missing key answers `401`;
//! * `GET /api/system/status` answers `{"data": {"bazarr_version": …, "sonarr_version": …,
//!   "radarr_version": …}}`;
//! * `GET /api/system/searches?query=` answers a bare JSON ARRAY of `{title, year, radarrId}` (a
//!   movie) or `{title, year, sonarrSeriesId}` (a show), a case- and accent-folded substring match;
//! * `GET /api/movies?radarrid[]=` and `GET /api/episodes?seriesid[]=` answer `{"data": […]}` rows
//!   carrying the file `path` (Bazarr's path, not Plex's: only the BASE NAME is compared),
//!   `imdbId` for a movie, `season`/`episode`/`sonarrEpisodeId` for an episode;
//! * `GET /api/providers/movies?radarrid=` and `GET /api/providers/episodes?episodeid=` search the
//!   providers SYNCHRONOUSLY (tens of seconds) and answer `{"data": […]}` rows of `provider`,
//!   `subtitle` (an opaque handle that only the download understands), `language`,
//!   `hearing_impaired`/`forced` (the STRINGS `"True"`/`"False"`), `score`, `release_info`;
//! * `POST /api/providers/movies` (`radarrid`) and `/episodes` (`seriesid`, `episodeid`) with the
//!   form fields `hi`, `forced`, `original_format`, `provider`, `subtitle` download the pick and
//!   answer `204`. Bazarr writes the file beside the video; PMS learns of it on a refresh.
//!
//! The module is pure but for [`CurlTransport`], the one place a socket is opened: everything else
//! takes a [`Transport`], so the tests drive the whole flow against a fake. The API key is only ever
//! put in a header; no request, reply or error value holds it, and nothing here logs a URL.

use super::models::Metadata;
use super::session::BazarrLink;
use serde_json::Value;

// ---- transport -----------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Get,
    Post,
}

/// One request. `path` starts with `/api/` and carries its own query string; `form` is a
/// url-encoded body for a POST. `slow` is a provider search or a download, which Bazarr answers
/// only when every provider has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub verb: Verb,
    pub path: String,
    pub form: Option<String>,
    pub slow: bool,
}

/// What came back: the status and the body. A request that never completed is a
/// [`TransportError`], not a reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// Nothing answered: no route, refused, a certificate this TV cannot verify, a reset.
    Unreachable,
    /// It answered too slowly.
    TimedOut,
}

pub trait Transport {
    fn send(&self, link: &BazarrLink, request: &Request) -> Result<Reply, TransportError>;
}

/// The real transport: libcurl through the `net` layer. `https://` is verified against the TV's
/// trust store first and, when that store cannot verify the chain (an older firmware without the
/// current Let's Encrypt root), against the bundled public roots; `http://` is plain.
pub struct CurlTransport;

impl Transport for CurlTransport {
    fn send(&self, link: &BazarrLink, request: &Request) -> Result<Reply, TransportError> {
        use plx_net::net::{self, RequestError, Tls, Timeouts};
        let url = format!("{}{}", link.url.trim_end_matches('/'), request.path);
        let mut headers = vec![format!("X-API-KEY: {}", link.api_key), "Accept: application/json".to_string()];
        let body: Option<Vec<u8>> = match (request.verb, &request.form) {
            (Verb::Post, Some(form)) => {
                headers.push("Content-Type: application/x-www-form-urlencoded".to_string());
                Some(form.clone().into_bytes())
            }
            (Verb::Post, None) => Some(Vec::new()),
            (Verb::Get, _) => None,
        };
        let timeouts = if request.slow {
            Timeouts { connect_s: 8, total_s: 150, total_ms: 0, low_speed_bps: 0, low_speed_s: 0 }
        } else {
            net::API
        };
        let verb = match request.verb {
            Verb::Get => "GET",
            Verb::Post => "POST",
        };
        let first = net::request_evidence(&url, &headers, verb, body.as_deref(), timeouts, false, Some(MAX_BODY), None);
        let failure = match first {
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

/// The biggest body read: a manual search over several providers is a few hundred rows at most.
const MAX_BODY: usize = 4 * 1024 * 1024;

// ---- what the viewer's item is, and what Bazarr offers ---------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Movie,
    Episode,
}

/// The facts of the playing item that find it in Bazarr: read from PMS, never from the screen.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ItemFacts {
    pub episode: bool,
    /// The movie's title, or the SHOW's title for an episode.
    pub title: String,
    pub year: i64,
    pub season: i64,
    pub number: i64,
    pub imdb: Option<String>,
    /// File base names of the item's parts, lower-cased.
    pub files: Vec<String>,
}

impl ItemFacts {
    pub fn kind(&self) -> ItemKind {
        if self.episode { ItemKind::Episode } else { ItemKind::Movie }
    }

    /// The facts out of a PMS item. `None` for a type Bazarr does not manage (a track, a clip).
    pub fn from_metadata(m: &Metadata) -> Option<Self> {
        let episode = match m.kind.as_str() {
            "movie" => false,
            "episode" => true,
            _ => return None,
        };
        let title = if episode { m.grandparent_title.clone() } else { m.title.clone() };
        if title.trim().is_empty() {
            return None;
        }
        let files = m
            .media
            .iter()
            .flat_map(|media| media.part.iter())
            .filter_map(|part| base_name(&part.file))
            .collect();
        let imdb = m.guids.iter().find_map(|g| g.of("imdb")).map(str::to_string);
        Some(Self {
            episode,
            title,
            // An episode's own `year` is its air date's, not the show's: it cannot tell two shows of
            // one title apart, so it is not used for that.
            year: if episode { 0 } else { m.year },
            season: m.parent_index.unwrap_or(0),
            number: m.index,
            imdb,
            files,
        })
    }
}

/// The last path component, lower-cased: Plex and Bazarr see the same file under different roots.
fn base_name(path: &str) -> Option<String> {
    let name = path.rsplit(['/', '\\']).next()?.trim();
    (!name.is_empty()).then(|| name.to_lowercase())
}

/// Where the item lives in Bazarr: the ids the providers and the download are addressed by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Movie { radarr_id: i64 },
    Episode { series_id: i64, episode_id: i64 },
}

/// One subtitle a provider offers.
#[derive(Clone, PartialEq, Eq)]
pub struct Candidate {
    pub provider: String,
    /// Bazarr's opaque handle for the download; valid for the search that produced it.
    pub subtitle: String,
    /// The release name the provider lists it under.
    pub release: String,
    /// Bazarr's language code as sent (`en`, `pt-BR`).
    pub language: String,
    pub hearing_impaired: bool,
    pub forced: bool,
    /// How well it matches the file, 0..=100 (Bazarr's own score).
    pub score: i64,
    pub original_format: bool,
}

impl std::fmt::Debug for Candidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // the handle can be kilobytes of opaque text: never print it
        write!(f, "Candidate {{ provider: {:?}, language: {:?}, score: {}, hi: {}, forced: {} }}", self.provider, self.language, self.score, self.hearing_impaired, self.forced)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub bazarr: String,
    pub sonarr: String,
    pub radarr: String,
}

/// Why a call did not produce what was asked. The screens say each differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BazarrError {
    /// No address or no key set.
    NotSet,
    /// The address did not answer.
    Unreachable,
    TimedOut,
    /// It answered `401`: the address is Bazarr (or something that checks keys), the key is wrong.
    BadKey,
    /// It answered, but not like Bazarr: HTML, an unexpected shape.
    NotBazarr,
    /// Bazarr does not hold this title, or it holds several and none is the file.
    NotInBazarr,
    /// Bazarr's own refusal (`500` with a message, such as a path-mapping fault).
    Refused,
}

// ---- the calls --------------------------------------------------------------------------------------

pub struct Bazarr<'a, T: Transport> {
    link: &'a BazarrLink,
    http: &'a T,
}

fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A JSON string field, tolerating a number or null.
fn text(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn int(v: &Value, key: &str) -> i64 {
    match v.get(key) {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// Bazarr sends flags as `"True"`/`"False"` strings on this endpoint and real booleans elsewhere.
fn flag(v: &Value, key: &str) -> bool {
    match v.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true") || s == "1",
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    }
}

/// `{"data": [...]}` rows, or a bare array.
fn rows(body: &[u8]) -> Option<Vec<Value>> {
    match serde_json::from_slice::<Value>(body).ok()? {
        Value::Array(a) => Some(a),
        Value::Object(mut o) => match o.remove("data")? {
            Value::Array(a) => Some(a),
            _ => None,
        },
        _ => None,
    }
}

/// The base letter of the common accented Latin letters. Bazarr folds a title with `unidecode`
/// before it compares, so "Amélie" and "Amelie" are one title there; PMS and Radarr may spell them
/// either way.
fn deaccent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' => 'c',
        'ď' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => 'e',
        'ğ' => 'g',
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => 'i',
        'ł' => 'l',
        'ñ' | 'ń' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => 'o',
        'ř' => 'r',
        'š' | 'ś' | 'ş' | 'ß' => 's',
        'ť' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => 'u',
        'ý' | 'ÿ' => 'y',
        'ž' | 'ź' | 'ż' => 'z',
        other => other,
    }
}

/// Fold for comparing titles: lower case, accents dropped, letters and digits only.
fn fold(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).map(deaccent).collect()
}

impl<'a, T: Transport> Bazarr<'a, T> {
    pub fn new(link: &'a BazarrLink, http: &'a T) -> Self {
        Self { link, http }
    }

    fn call(&self, request: Request) -> Result<Reply, BazarrError> {
        if !self.link.is_complete() {
            return Err(BazarrError::NotSet);
        }
        let reply = self.http.send(self.link, &request).map_err(|e| match e {
            TransportError::Unreachable => BazarrError::Unreachable,
            TransportError::TimedOut => BazarrError::TimedOut,
        })?;
        match reply.status {
            200..=299 => Ok(reply),
            401 | 403 => Err(BazarrError::BadKey),
            404 => Err(BazarrError::NotInBazarr),
            500..=599 => Err(BazarrError::Refused),
            _ => Err(BazarrError::NotBazarr),
        }
    }

    fn get(&self, path: String, slow: bool) -> Result<Reply, BazarrError> {
        self.call(Request { verb: Verb::Get, path, form: None, slow })
    }

    /// `GET /api/system/status`: is there a Bazarr at this address that accepts this key?
    pub fn status(&self) -> Result<Status, BazarrError> {
        let reply = self.get("/api/system/status".into(), false)?;
        let v: Value = serde_json::from_slice(&reply.body).map_err(|_| BazarrError::NotBazarr)?;
        let data = v.get("data").filter(|d| d.is_object()).ok_or(BazarrError::NotBazarr)?;
        let bazarr = text(data, "bazarr_version");
        if bazarr.is_empty() {
            return Err(BazarrError::NotBazarr);
        }
        Ok(Status { bazarr, sonarr: text(data, "sonarr_version"), radarr: text(data, "radarr_version") })
    }

    /// Find the item in Bazarr. A movie is matched on title and year, then on the file name when
    /// several share them; a show on title, then the season and episode number, then the file name.
    pub fn resolve(&self, item: &ItemFacts) -> Result<Target, BazarrError> {
        let reply = self.get(format!("/api/system/searches?query={}", enc(&item.title)), false)?;
        let found = rows(&reply.body).ok_or(BazarrError::NotBazarr)?;
        let wanted = fold(&item.title);
        let same_title: Vec<&Value> = found.iter().filter(|r| fold(&text(r, "title")) == wanted).collect();
        match item.kind() {
            ItemKind::Movie => {
                let movies: Vec<&&Value> = same_title.iter().filter(|r| r.get("radarrId").is_some()).collect();
                let by_year: Vec<&&&Value> = movies.iter().filter(|r| item.year == 0 || int(r, "year") == item.year).collect();
                let pool: Vec<i64> = if by_year.is_empty() { movies.iter().map(|r| int(r, "radarrId")).collect() } else { by_year.iter().map(|r| int(r, "radarrId")).collect() };
                match pool.as_slice() {
                    [] => Err(BazarrError::NotInBazarr),
                    [only] => Ok(Target::Movie { radarr_id: *only }),
                    several => self.movie_by_file(several, item),
                }
            }
            ItemKind::Episode => {
                let series: Vec<i64> = same_title.iter().filter(|r| r.get("sonarrSeriesId").is_some()).map(|r| int(r, "sonarrSeriesId")).collect();
                if series.is_empty() {
                    return Err(BazarrError::NotInBazarr);
                }
                self.episode_in(&series, item)
            }
        }
    }

    fn movie_by_file(&self, ids: &[i64], item: &ItemFacts) -> Result<Target, BazarrError> {
        for id in ids {
            let reply = self.get(format!("/api/movies?radarrid%5B%5D={id}"), false)?;
            let movies = rows(&reply.body).ok_or(BazarrError::NotBazarr)?;
            let hit = movies.iter().any(|m| {
                let by_file = base_name(&text(m, "path")).is_some_and(|f| item.files.contains(&f));
                let by_imdb = item.imdb.as_deref().is_some_and(|i| !i.is_empty() && i == text(m, "imdbId"));
                by_file || by_imdb
            });
            if hit {
                return Ok(Target::Movie { radarr_id: *id });
            }
        }
        Err(BazarrError::NotInBazarr)
    }

    fn episode_in(&self, series: &[i64], item: &ItemFacts) -> Result<Target, BazarrError> {
        for series_id in series {
            let reply = self.get(format!("/api/episodes?seriesid%5B%5D={series_id}"), false)?;
            let episodes = rows(&reply.body).ok_or(BazarrError::NotBazarr)?;
            let by_number = episodes.iter().find(|e| int(e, "season") == item.season && int(e, "episode") == item.number);
            let by_file = episodes.iter().find(|e| base_name(&text(e, "path")).is_some_and(|f| item.files.contains(&f)));
            if let Some(e) = by_file.or(by_number) {
                return Ok(Target::Episode { series_id: *series_id, episode_id: int(e, "sonarrEpisodeId") });
            }
        }
        Err(BazarrError::NotInBazarr)
    }

    /// Ask every provider for subtitles for the item. Slow: Bazarr answers when the last provider
    /// has. The rows come back best score first, then by provider and release for a stable order.
    pub fn search(&self, target: Target) -> Result<Vec<Candidate>, BazarrError> {
        let path = match target {
            Target::Movie { radarr_id } => format!("/api/providers/movies?radarrid={radarr_id}"),
            Target::Episode { episode_id, .. } => format!("/api/providers/episodes?episodeid={episode_id}"),
        };
        let reply = self.get(path, true)?;
        let found = rows(&reply.body).ok_or(BazarrError::NotBazarr)?;
        let mut out: Vec<Candidate> = found
            .iter()
            .filter_map(|r| {
                let provider = text(r, "provider");
                let subtitle = text(r, "subtitle");
                if provider.is_empty() || subtitle.is_empty() {
                    return None;
                }
                let release = match r.get("release_info") {
                    Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).next().unwrap_or("").to_string(),
                    Some(Value::String(s)) => s.clone(),
                    _ => String::new(),
                };
                Some(Candidate {
                    provider,
                    subtitle,
                    release: if release.is_empty() { text(r, "uploader") } else { release },
                    language: text(r, "language"),
                    hearing_impaired: flag(r, "hearing_impaired"),
                    forced: flag(r, "forced"),
                    score: int(r, "score").clamp(0, 100),
                    original_format: flag(r, "original_format"),
                })
            })
            .collect();
        out.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.provider.cmp(&b.provider)).then_with(|| a.release.cmp(&b.release)));
        Ok(out)
    }

    /// Download one candidate. Bazarr saves the file beside the video and answers `204`.
    pub fn download(&self, target: Target, candidate: &Candidate) -> Result<(), BazarrError> {
        let b = |v: bool| if v { "True" } else { "False" };
        let mut form = match target {
            Target::Movie { radarr_id } => format!("radarrid={radarr_id}"),
            Target::Episode { series_id, episode_id } => format!("seriesid={series_id}&episodeid={episode_id}"),
        };
        form.push_str(&format!(
            "&hi={}&forced={}&original_format={}&provider={}&subtitle={}",
            b(candidate.hearing_impaired),
            b(candidate.forced),
            b(candidate.original_format),
            enc(&candidate.provider),
            enc(&candidate.subtitle)
        ));
        let path = match target {
            Target::Movie { .. } => "/api/providers/movies",
            Target::Episode { .. } => "/api/providers/episodes",
        };
        self.call(Request { verb: Verb::Post, path: path.to_string(), form: Some(form), slow: true }).map(|_| ())
    }
}

/// Keep the candidates in `language` (a 2-letter primary subtag, or a longer tag whose primary
/// subtag it is): Bazarr returns every language of the title's profile in one list.
pub fn in_language(candidates: Vec<Candidate>, language: &str) -> Vec<Candidate> {
    let want = language.trim().split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    if want.is_empty() {
        return candidates;
    }
    candidates
        .into_iter()
        .filter(|c| c.language.split(['-', '_']).next().unwrap_or("").eq_ignore_ascii_case(&want))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A fake Bazarr: answers by exact path, records every request it was sent.
    struct Fake {
        routes: Vec<(String, Result<Reply, TransportError>)>,
        seen: RefCell<Vec<Request>>,
        keys: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(routes: Vec<(&str, u16, &str)>) -> Self {
            Self {
                routes: routes.into_iter().map(|(p, s, b)| (p.to_string(), Ok(Reply { status: s, body: b.as_bytes().to_vec() }))).collect(),
                seen: RefCell::new(Vec::new()),
                keys: RefCell::new(Vec::new()),
            }
        }
        fn dead() -> Self {
            Self { routes: vec![("/api/system/status".into(), Err(TransportError::Unreachable))], seen: RefCell::new(Vec::new()), keys: RefCell::new(Vec::new()) }
        }
    }

    impl Transport for Fake {
        fn send(&self, link: &BazarrLink, request: &Request) -> Result<Reply, TransportError> {
            self.keys.borrow_mut().push(link.api_key.clone());
            self.seen.borrow_mut().push(request.clone());
            self.routes.iter().find(|(p, _)| *p == request.path).map(|(_, r)| r.clone()).unwrap_or(Ok(Reply { status: 404, body: Vec::new() }))
        }
    }

    fn link() -> BazarrLink {
        BazarrLink { url: "https://bazarr.example.net".into(), api_key: "k".into() }
    }

    const MOVIE_SEARCH: &str = r#"[{"title":"Tears of Steel","year":"2012","radarrId":7,"poster":null},
        {"title":"Tears of Steel","year":"1999","radarrId":8,"poster":null},
        {"title":"Tears of Steel: Behind","year":"2012","radarrId":9,"poster":null}]"#;

    fn movie() -> ItemFacts {
        ItemFacts { title: "Tears of Steel".into(), year: 2012, files: vec!["tears.of.steel.2012.mkv".into()], ..Default::default() }
    }

    #[test]
    fn status_reads_the_versions_and_tells_a_wrong_key_from_a_wrong_address() {
        let ok = Fake::new(vec![("/api/system/status", 200, r#"{"data":{"bazarr_version":"1.5.1","sonarr_version":"4.0.9","radarr_version":""}}"#)]);
        assert_eq!(
            Bazarr::new(&link(), &ok).status(),
            Ok(Status { bazarr: "1.5.1".into(), sonarr: "4.0.9".into(), radarr: String::new() })
        );
        let bad_key = Fake::new(vec![("/api/system/status", 401, "{}")]);
        assert_eq!(Bazarr::new(&link(), &bad_key).status(), Err(BazarrError::BadKey));
        let html = Fake::new(vec![("/api/system/status", 200, "<html>login</html>")]);
        assert_eq!(Bazarr::new(&link(), &html).status(), Err(BazarrError::NotBazarr));
        let no_version = Fake::new(vec![("/api/system/status", 200, r#"{"data":{}}"#)]);
        assert_eq!(Bazarr::new(&link(), &no_version).status(), Err(BazarrError::NotBazarr));
        assert_eq!(Bazarr::new(&link(), &Fake::dead()).status(), Err(BazarrError::Unreachable));
        assert_eq!(Bazarr::new(&BazarrLink::default(), &ok).status(), Err(BazarrError::NotSet), "nothing is sent without a link");
        assert!(ok.seen.borrow().len() == 1, "only the first fake saw a request");
    }

    #[test]
    fn a_movie_is_found_by_title_and_year() {
        let fake = Fake::new(vec![("/api/system/searches?query=Tears%20of%20Steel", 200, MOVIE_SEARCH)]);
        assert_eq!(Bazarr::new(&link(), &fake).resolve(&movie()), Ok(Target::Movie { radarr_id: 7 }));
        assert_eq!(fake.seen.borrow().len(), 1, "a unique title+year needs no second look");
        assert!(fake.keys.borrow().iter().all(|k| k == "k"));
    }

    #[test]
    fn two_movies_of_one_title_and_year_are_told_apart_by_the_file() {
        let search = r#"[{"title":"Heat","year":"1995","radarrId":1},{"title":"Heat","year":"1995","radarrId":2}]"#;
        let fake = Fake::new(vec![
            ("/api/system/searches?query=Heat", 200, search),
            ("/api/movies?radarrid%5B%5D=1", 200, r#"{"data":[{"path":"/movies/Heat.1995.DC.mkv","imdbId":"tt1"}],"total":1}"#),
            ("/api/movies?radarrid%5B%5D=2", 200, r#"{"data":[{"path":"/data/media/Heat (1995)/Heat.1995.mkv","imdbId":"tt2"}],"total":1}"#),
        ]);
        let item = ItemFacts { title: "Heat".into(), year: 1995, files: vec!["heat.1995.mkv".into()], ..Default::default() };
        assert_eq!(Bazarr::new(&link(), &fake).resolve(&item), Ok(Target::Movie { radarr_id: 2 }));
        let by_imdb = ItemFacts { title: "Heat".into(), year: 1995, imdb: Some("tt1".into()), ..Default::default() };
        assert_eq!(Bazarr::new(&link(), &fake).resolve(&by_imdb), Ok(Target::Movie { radarr_id: 1 }));
    }

    #[test]
    fn a_title_bazarr_does_not_hold_is_not_in_bazarr() {
        let fake = Fake::new(vec![("/api/system/searches?query=Tears%20of%20Steel", 200, "[]")]);
        assert_eq!(Bazarr::new(&link(), &fake).resolve(&movie()), Err(BazarrError::NotInBazarr));
        let only_a_show = Fake::new(vec![("/api/system/searches?query=Tears%20of%20Steel", 200, r#"[{"title":"Tears of Steel","year":"2012","sonarrSeriesId":3}]"#)]);
        assert_eq!(Bazarr::new(&link(), &only_a_show).resolve(&movie()), Err(BazarrError::NotInBazarr), "a series is not a movie");
    }

    #[test]
    fn an_episode_is_found_by_show_season_and_number() {
        let fake = Fake::new(vec![
            ("/api/system/searches?query=Show%20Name", 200, r#"[{"title":"Show Name","year":"2010","sonarrSeriesId":11}]"#),
            (
                "/api/episodes?seriesid%5B%5D=11",
                200,
                r#"{"data":[{"season":1,"episode":1,"sonarrEpisodeId":100,"path":"/tv/a.mkv"},{"season":2,"episode":3,"sonarrEpisodeId":123,"path":"/tv/b.mkv"}]}"#,
            ),
        ]);
        let item = ItemFacts { episode: true, title: "Show Name".into(), season: 2, number: 3, ..Default::default() };
        assert_eq!(Bazarr::new(&link(), &fake).resolve(&item), Ok(Target::Episode { series_id: 11, episode_id: 123 }));
        let missing = ItemFacts { episode: true, title: "Show Name".into(), season: 9, number: 9, ..Default::default() };
        assert_eq!(Bazarr::new(&link(), &fake).resolve(&missing), Err(BazarrError::NotInBazarr));
    }

    #[test]
    fn search_reads_rows_with_string_flags_and_ranks_them() {
        let body = r#"{"data":[
            {"provider":"podnapisi","subtitle":"AAA","language":"en","hearing_impaired":"True","forced":"False","score":91,"release_info":["Tears.720p"],"uploader":"x"},
            {"provider":"opensubtitlescom","subtitle":"BBB","language":"en","hearing_impaired":"False","forced":"False","score":98,"release_info":["Tears.1080p","other"]},
            {"provider":"subf2m","subtitle":"CCC","language":"nl","hearing_impaired":false,"forced":true,"score":"84","release_info":[],"uploader":"up"},
            {"provider":"","subtitle":"DDD","language":"en","score":99},
            {"provider":"gestdown","subtitle":"","language":"en","score":99}]}"#;
        let fake = Fake::new(vec![("/api/providers/movies?radarrid=7", 200, body)]);
        let found = Bazarr::new(&link(), &fake).search(Target::Movie { radarr_id: 7 }).unwrap();
        let order: Vec<(&str, i64)> = found.iter().map(|c| (c.provider.as_str(), c.score)).collect();
        assert_eq!(order, [("opensubtitlescom", 98), ("podnapisi", 91), ("subf2m", 84)], "best first; rows with no provider or handle are dropped");
        assert!(found[1].hearing_impaired && !found[0].hearing_impaired && found[2].forced);
        assert_eq!(found[0].release, "Tears.1080p");
        assert_eq!(found[2].release, "up", "no release name: the uploader stands in");
        assert!(fake.seen.borrow()[0].slow, "a provider search is the slow call");
        let en = in_language(found, "en-GB");
        assert_eq!(en.len(), 2);
        assert!(!format!("{:?}", en[0]).contains("AAA") && !format!("{:?}", en[0]).contains("BBB"), "the handle never prints");
    }

    #[test]
    fn an_episode_search_is_by_episode_id() {
        let fake = Fake::new(vec![("/api/providers/episodes?episodeid=123", 200, r#"{"data":[]}"#)]);
        assert_eq!(Bazarr::new(&link(), &fake).search(Target::Episode { series_id: 11, episode_id: 123 }), Ok(Vec::new()));
        let server_fault = Fake::new(vec![("/api/providers/movies?radarrid=7", 500, "Movie file not found. Path mapping issue?")]);
        assert_eq!(Bazarr::new(&link(), &server_fault).search(Target::Movie { radarr_id: 7 }), Err(BazarrError::Refused));
    }

    #[test]
    fn a_download_posts_the_form_bazarr_documents_and_encodes_the_handle() {
        let fake = Fake::new(vec![("/api/providers/movies", 204, "")]);
        let c = Candidate {
            provider: "opensubtitlescom".into(),
            subtitle: "gASV+/=\nxyz&a b".into(),
            release: "r".into(),
            language: "en".into(),
            hearing_impaired: true,
            forced: false,
            score: 98,
            original_format: false,
        };
        assert_eq!(Bazarr::new(&link(), &fake).download(Target::Movie { radarr_id: 7 }, &c), Ok(()));
        let sent = fake.seen.borrow()[0].clone();
        assert_eq!(sent.verb, Verb::Post);
        assert!(sent.slow);
        assert_eq!(
            sent.form.as_deref(),
            Some("radarrid=7&hi=True&forced=False&original_format=False&provider=opensubtitlescom&subtitle=gASV%2B%2F%3D%0Axyz%26a%20b")
        );
        let ep = Fake::new(vec![("/api/providers/episodes", 204, "")]);
        assert_eq!(Bazarr::new(&link(), &ep).download(Target::Episode { series_id: 11, episode_id: 123 }, &c), Ok(()));
        assert!(ep.seen.borrow()[0].form.as_deref().unwrap().starts_with("seriesid=11&episodeid=123&hi=True"));
    }

    #[test]
    fn a_refused_download_is_not_a_success() {
        let denied = Fake::new(vec![("/api/providers/movies", 401, "")]);
        let c = Candidate { provider: "p".into(), subtitle: "s".into(), release: String::new(), language: "en".into(), hearing_impaired: false, forced: false, score: 1, original_format: false };
        assert_eq!(Bazarr::new(&link(), &denied).download(Target::Movie { radarr_id: 1 }, &c), Err(BazarrError::BadKey));
        let broken = Fake::new(vec![("/api/providers/movies", 500, "boom")]);
        assert_eq!(Bazarr::new(&link(), &broken).download(Target::Movie { radarr_id: 1 }, &c), Err(BazarrError::Refused));
    }

    #[test]
    fn item_facts_come_from_pms_and_only_for_movies_and_episodes() {
        let movie: Metadata = serde_json::from_str(
            r#"{"type":"movie","title":"Tears of Steel","year":2012,"Guid":[{"id":"imdb://tt2285752"},{"id":"tmdb://133701"}],
                "Media":[{"Part":[{"file":"/mnt/user/Movies/Tears of Steel (2012)/Tears.of.Steel.2012.MKV"}]}]}"#,
        )
        .unwrap();
        let facts = ItemFacts::from_metadata(&movie).unwrap();
        assert_eq!((facts.kind(), facts.year, facts.imdb.as_deref()), (ItemKind::Movie, 2012, Some("tt2285752")));
        assert_eq!(facts.files, ["tears.of.steel.2012.mkv"]);
        let episode: Metadata = serde_json::from_str(
            r#"{"type":"episode","title":"Pilot","grandparentTitle":"Show Name","year":2011,"parentIndex":2,"index":3,"Media":[{"Part":[{"file":"C:\\tv\\S02E03.mkv"}]}]}"#,
        )
        .unwrap();
        let e = ItemFacts::from_metadata(&episode).unwrap();
        assert_eq!((e.kind(), e.title.as_str(), e.season, e.number, e.year), (ItemKind::Episode, "Show Name", 2, 3, 0));
        assert_eq!(e.files, ["s02e03.mkv"], "a Windows path reads the same way");
        assert!(ItemFacts::from_metadata(&serde_json::from_str::<Metadata>(r#"{"type":"track","title":"x"}"#).unwrap()).is_none());
    }

    #[test]
    fn titles_compare_without_case_accents_free_punctuation() {
        assert_eq!(fold("Tears of Steel!"), fold("tears  of steel"));
        assert_eq!(fold("Amélie"), fold("AMELIE"));
        assert_eq!(fold("Léon: The Professional"), fold("leon the professional"));
        assert_ne!(fold("Heat"), fold("Heat 2"));
    }

    /// **Live smoke test, read-only.** Against a real Bazarr: its status, then the first movie and the
    /// first episode it holds are found again from the facts a PMS item would carry, and the movie's
    /// providers are asked. No download, no delete. The key is read from a FILE and never printed:
    ///
    /// `BAZARR_SMOKE_URL=https://bazarr.example.net BAZARR_SMOKE_KEY_FILE=/path/to/key cargo test -p
    /// plx_plex --lib live_smoke -- --ignored --nocapture`
    #[test]
    #[ignore = "talks to a real Bazarr: BAZARR_SMOKE_URL and BAZARR_SMOKE_KEY_FILE"]
    fn live_smoke_status_resolve_and_search() {
        assert!(plx_net::net::global_init(), "libcurl binds");
        let url = std::env::var("BAZARR_SMOKE_URL").expect("BAZARR_SMOKE_URL");
        let key = std::fs::read_to_string(std::env::var("BAZARR_SMOKE_KEY_FILE").expect("BAZARR_SMOKE_KEY_FILE")).expect("the key file");
        let link = BazarrLink {
            url: BazarrLink::normalize_url(&url).expect("a usable address"),
            api_key: BazarrLink::normalize_key(&key).expect("a usable key"),
        };
        let http = CurlTransport;
        let bazarr = Bazarr::new(&link, &http);
        let status = bazarr.status().expect("status");
        println!("bazarr {} / sonarr {} / radarr {}", status.bazarr, status.sonarr, status.radarr);

        let get = |path: &str| -> Value {
            let reply = http.send(&link, &Request { verb: Verb::Get, path: path.to_string(), form: None, slow: false }).expect("reachable");
            assert_eq!(reply.status, 200, "{path}");
            serde_json::from_slice(&reply.body).expect("JSON")
        };

        // a movie, described as a PMS item would describe it
        let movies = get("/api/movies?start=0&length=1");
        let movie = &movies["data"][0];
        let movie_facts = ItemFacts {
            episode: false,
            title: text(movie, "title"),
            year: text(movie, "year").parse().unwrap_or(0),
            imdb: Some(text(movie, "imdbId")).filter(|s| !s.is_empty()),
            files: base_name(&text(movie, "path")).into_iter().collect(),
            ..Default::default()
        };
        let target = bazarr.resolve(&movie_facts).expect("the movie is found again");
        println!("movie {:?} ({}) resolves to {target:?}", movie_facts.title, movie_facts.year);

        // an episode of the first series
        let series = get("/api/series?start=0&length=1");
        let show = &series["data"][0];
        let episodes = get(&format!("/api/episodes?seriesid%5B%5D={}", text(show, "sonarrSeriesId")));
        let episode = episodes["data"].as_array().and_then(|rows| rows.iter().find(|e| !text(e, "path").is_empty())).expect("an episode with a file");
        let episode_facts = ItemFacts {
            episode: true,
            title: text(show, "title"),
            season: text(episode, "season").parse().unwrap_or(0),
            number: text(episode, "episode").parse().unwrap_or(0),
            files: base_name(&text(episode, "path")).into_iter().collect(),
            ..Default::default()
        };
        let episode_target = bazarr.resolve(&episode_facts).expect("the episode is found again");
        println!("episode {:?} S{:02}E{:02} resolves to {episode_target:?}", episode_facts.title, episode_facts.season, episode_facts.number);

        // the providers, for the movie only: one search is quota enough for a smoke test
        let started = std::time::Instant::now();
        let candidates = bazarr.search(target).expect("the providers answer");
        println!("{} candidates in {:.1}s", candidates.len(), started.elapsed().as_secs_f32());
        for c in candidates.iter().take(3) {
            println!("  {:>3} {:<14} {:<6} hi={} forced={} {}", c.score, c.provider, c.language, c.hearing_impaired, c.forced, c.release);
        }
        assert!(candidates.iter().all(|c| !c.subtitle.is_empty() && !c.provider.is_empty()), "every candidate carries a handle and a provider");
    }
}
