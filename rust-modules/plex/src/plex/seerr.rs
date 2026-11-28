//! **Seerr's HTTP API** (the Overseerr v1 API Seerr 3.5 serves), as far as the Requests area needs
//! it. The contract is Seerr's own source (`server/routes/request.ts`, `server/entity/MediaRequest.ts`,
//! read 2026-11-17) and replies recorded from a 3.5.2 server (`tests/fixtures/seerr/`):
//!
//! * `POST /api/v1/auth/plex {authToken}` signs a Plex account in and sets the session cookie every
//!   other call carries; a call without one answers `401`;
//! * `GET /api/v1/status` (no session) answers `{version}`; `/auth/me` the signed-in user;
//!   `/settings/public` whether a request may name some seasons and whether specials count;
//! * `GET /api/v1/discover/{trending,movies,tv,movies/upcoming}?page=` and `/search?query=&page=`
//!   answer `{page, totalPages, results}`; each result names its `mediaType` and, once Seerr knows
//!   the title, a `mediaInfo` whose `status` is all a tile shows (its `requests` are empty here);
//! * `GET /api/v1/movie/{id}` and `/tv/{id}` carry `mediaInfo.requests` and each season's status;
//! * `GET /api/v1/request?requestedBy=&take=&skip=&sort=added` answers `{pageInfo, results}`, each
//!   request naming its title by id only;
//! * `POST /api/v1/request {mediaType, mediaId[, seasons]}` answers `201`; `202` when no season was
//!   left to ask for, `409` for a duplicate, `403` for a missing permission, a spent quota or a
//!   blocklisted title, `429` when a proxy in front rate-limits.
//!
//! **A duplicate is never sent.** Pulsarr turns a Watchlist add into a request on its own, so
//! [`Detail::ask`] applies Seerr's own rules (an active request of the title or the season, or media
//! Seerr already holds) before the page offers Request, and [`Seerr::request`] reads the title
//! again and refuses when the answer changed in between.
//!
//! The module is pure but for [`CurlTransport`]: everything else takes a [`Transport`], so the tests
//! drive the flows against recorded replies. The session cookie and the Plex token only travel in a
//! header or the sign-in body; no error value holds them and nothing here logs a URL.

use super::bazarr::{enc, int, text};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::cell::RefCell;

mod title_info;
pub use title_info::{is_video_key, Info, Person, Role, SeasonArt, Stage, Still, Video, VideoKind};

// ---- transport -----------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Get,
    Post,
    Delete,
}

/// One request. `path` starts with `/api/v1/` and carries its own query string; `json` is a POST's
/// body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub verb: Verb,
    pub path: String,
    pub json: Option<String>,
}

/// What came back: the status, the body and the `name=value` of each cookie it set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    pub cookies: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// Nothing answered: no route, refused, a certificate this TV cannot verify, a reset.
    Unreachable,
    TimedOut,
}

pub trait Transport {
    /// Send `request` to the Seerr at `base`, carrying `cookie` (empty for none).
    fn send(&self, base: &str, cookie: &str, request: &Request) -> Result<Reply, TransportError>;
}

/// The real transport: libcurl through the `net` layer, verified against the TV's trust store and,
/// when that cannot verify the chain, the bundled public roots (as Bazarr's).
pub struct CurlTransport;

impl Transport for CurlTransport {
    fn send(&self, base: &str, cookie: &str, request: &Request) -> Result<Reply, TransportError> {
        use plx_net::net::{self, RequestError, Tls};
        let url = format!("{}{}", base.trim_end_matches('/'), request.path);
        let mut headers = vec!["Accept: application/json".to_string()];
        if !cookie.is_empty() {
            headers.push(format!("Cookie: {cookie}"));
        }
        let body = request.json.as_ref().map(|json| {
            headers.push("Content-Type: application/json".to_string());
            json.clone().into_bytes()
        });
        let verb = match request.verb {
            Verb::Get => "GET",
            Verb::Post => "POST",
            Verb::Delete => "DELETE",
        };
        let reply = |resp: net::Resp| Reply { status: resp.status, body: resp.body, cookies: resp.cookies };
        let failure = match net::request_evidence(&url, &headers, verb, body.as_deref(), net::API, false, Some(MAX_BODY), None) {
            Ok(resp) => return Ok(reply(resp)),
            Err(failure) => failure,
        };
        if failure.untrusted_chain().is_some() {
            let bundle = plx_base::paths::app_dir().join(net::keypin::ROOTS_FILE);
            if let Some(path) = bundle.to_str().filter(|_| bundle.is_file()) {
                if let Some(resp) = net::request_tls(&url, &headers, verb, body.as_deref(), net::API, false, Some(MAX_BODY), Tls::CaBundle(path)) {
                    return Ok(reply(resp));
                }
            }
        }
        match failure.cause {
            RequestError::TimedOut => Err(TransportError::TimedOut),
            RequestError::Transport => Err(TransportError::Unreachable),
        }
    }
}

/// The biggest body read: a title with every season and its credits is a few hundred kilobytes.
const MAX_BODY: usize = 4 * 1024 * 1024;
/// Requests read per call of [`Seerr::my_requests`]; the next batch is read when the row asks.
pub const REQUEST_PAGE: u32 = 20;

// ---- what Seerr says ---------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Movie,
    Tv,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Movie => "movie",
            Kind::Tv => "tv",
        }
    }
}

/// Seerr's media status (`MediaStatus`): how much of a title (or a season) it holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Have {
    /// Seerr knows nothing, or it was removed (`UNKNOWN`, `DELETED`): it may be asked for.
    #[default]
    Nothing,
    /// Asked for, waiting for approval.
    Pending,
    /// Approved and on its way.
    Processing,
    Partly,
    All,
    Blocklisted,
}

impl Have {
    pub(super) fn of(code: i64) -> Self {
        match code {
            2 => Have::Pending,
            3 => Have::Processing,
            4 => Have::Partly,
            5 => Have::All,
            6 => Have::Blocklisted,
            _ => Have::Nothing,
        }
    }
}

/// The word a tile or a page shows about a title.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Badge {
    Requested,
    Approved,
    Downloading,
    PartlyAvailable,
    Available,
    Blocked,
    Declined,
    Failed,
}

/// What Seerr holds of a title and whether a download is running.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaState {
    pub have: Have,
    pub downloading: bool,
}

impl MediaState {
    fn of(info: &Value, is4k: bool) -> Self {
        let have = Have::of(int(info, if is4k { "status4k" } else { "status" }));
        let key = if is4k { "downloadStatus4k" } else { "downloadStatus" };
        let downloading = info.get(key).and_then(Value::as_array).is_some_and(|d| !d.is_empty());
        Self { have, downloading }
    }

    pub fn badge(self) -> Option<Badge> {
        match self.have {
            Have::Pending => Some(Badge::Requested),
            Have::Processing if self.downloading => Some(Badge::Downloading),
            Have::Processing => Some(Badge::Approved),
            Have::Partly => Some(Badge::PartlyAvailable),
            Have::All => Some(Badge::Available),
            Have::Blocklisted => Some(Badge::Blocked),
            Have::Nothing => None,
        }
    }
}

/// A title as a row or a page shows it. `poster` and `backdrop` are TMDB image paths (see
/// [`tmdb_image`]), empty when TMDB has none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Title {
    pub kind: Kind,
    pub tmdb: u32,
    pub name: String,
    pub year: String,
    pub poster: String,
    pub backdrop: String,
    pub overview: String,
    pub state: MediaState,
}

impl Title {
    fn of(v: &Value, kind: Kind) -> Option<Self> {
        let tmdb = u32::try_from(int(v, "id")).ok().filter(|id| *id > 0)?;
        let (name, date) = match kind {
            Kind::Movie => (text(v, "title"), text(v, "releaseDate")),
            Kind::Tv => (text(v, "name"), text(v, "firstAirDate")),
        };
        Some(Self {
            kind,
            tmdb,
            name,
            year: date.get(..4).filter(|y| y.bytes().all(|b| b.is_ascii_digit())).unwrap_or("").to_string(),
            poster: text(v, "posterPath"),
            backdrop: text(v, "backdropPath"),
            overview: text(v, "overview"),
            state: v.get("mediaInfo").map(|info| MediaState::of(info, false)).unwrap_or_default(),
        })
    }

    /// A search or discover result: movies and shows, never people.
    fn of_result(v: &Value) -> Option<Self> {
        match v.get("mediaType").and_then(Value::as_str)? {
            "movie" => Self::of(v, Kind::Movie),
            "tv" => Self::of(v, Kind::Tv),
            _ => None,
        }
    }
}

/// A TMDB image at one of its published widths (`w342`, `w780`, `w1280`, `original`); empty
/// when there is no image.
pub fn tmdb_image(path: &str, width: &str) -> String {
    if path.starts_with('/') {
        format!("https://image.tmdb.org/t/p/{width}{path}")
    } else {
        String::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: u32,
    pub pages: u32,
}

impl<T> Page<T> {
    pub fn more(&self) -> bool {
        self.page < self.pages
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shelf {
    Trending,
    Movies,
    Shows,
    Upcoming,
}

impl Shelf {
    pub const ALL: [Shelf; 4] = [Shelf::Trending, Shelf::Movies, Shelf::Shows, Shelf::Upcoming];

    /// This shelf's place in [`Shelf::ALL`].
    pub fn index(self) -> usize {
        match self {
            Shelf::Trending => 0,
            Shelf::Movies => 1,
            Shelf::Shows => 2,
            Shelf::Upcoming => 3,
        }
    }

    fn path(self) -> &'static str {
        match self {
            Shelf::Trending => "/api/v1/discover/trending",
            Shelf::Movies => "/api/v1/discover/movies",
            Shelf::Shows => "/api/v1/discover/tv",
            Shelf::Upcoming => "/api/v1/discover/movies/upcoming",
        }
    }
}

/// A movie genre as Seerr lists it, named in the language the Seerr user reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Genre {
    pub id: u32,
    pub name: String,
}

/// The order a filtered movie listing is read in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sort {
    Popular,
    TopRated,
    Newest,
}

impl Sort {
    pub const ALL: [Sort; 3] = [Sort::Popular, Sort::TopRated, Sort::Newest];
}

/// The filters of a movie listing, all of which TMDB applies before it pages: a genre, a keyword (a
/// mood such as "heist"), and an inclusive range of release years. `None` leaves that side open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Browse {
    pub genre: Option<u32>,
    pub keyword: Option<u32>,
    pub from: Option<u16>,
    pub to: Option<u16>,
}

impl Browse {
    pub fn is_empty(&self) -> bool {
        *self == Browse::default()
    }

    /// `GET /api/v1/discover/movies` for one page (from 1) in `sort` order. `today` is `YYYY-MM-DD`:
    /// the upper bound that keeps unreleased titles out of the newest-first order, and the end of a
    /// range that has only a first year. A last year alone starts at 1900. Top rated needs a vote floor,
    /// as otherwise a title with a handful of perfect votes leads; newest-first needs a small one for
    /// the same reason. The `language` parameter is never sent: Seerr reads it as the ORIGINAL language.
    pub fn path(&self, sort: Sort, page: u32, today: &str) -> String {
        let order = match sort {
            Sort::Popular => "popularity.desc",
            Sort::TopRated => "vote_average.desc",
            Sort::Newest => "primary_release_date.desc",
        };
        let mut path = format!("/api/v1/discover/movies?page={page}&sortBy={order}");
        if let Some(genre) = self.genre {
            path.push_str(&format!("&genre={genre}"));
        }
        if let Some(keyword) = self.keyword {
            path.push_str(&format!("&keywords={keyword}"));
        }
        let from = self.from.map(|y| format!("{y:04}-01-01"));
        let to = self.to.map(|y| format!("{y:04}-12-31"));
        let first = from.clone().or_else(|| to.is_some().then(|| "1900-01-01".to_string()));
        let last = match sort {
            Sort::Newest => Some(to.filter(|end| end.as_str() < today).unwrap_or_else(|| today.to_string())),
            _ => to.or_else(|| from.is_some().then(|| today.to_string())),
        };
        if let Some(first) = first {
            path.push_str(&format!("&primaryReleaseDateGte={first}"));
        }
        if let Some(last) = last {
            path.push_str(&format!("&primaryReleaseDateLte={last}"));
        }
        match sort {
            Sort::Popular => {}
            Sort::TopRated => path.push_str("&voteCountGte=100"),
            Sort::Newest => path.push_str("&voteCountGte=5"),
        }
        path
    }
}

/// One season of a show as the request picker needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Season {
    pub number: u32,
    pub episodes: u32,
    pub have: Have,
    /// Under a request that is still open (pending or approved).
    pub requested: bool,
}

/// Where the title sits in Plex, when Seerr has matched it there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InPlex {
    /// The server's `machineIdentifier`.
    pub machine: String,
    pub rating_key: String,
}

/// What Seerr's request rules allow for a title right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Request sends the movie.
    Movie,
    /// These seasons may be asked for, in order.
    Seasons(Vec<u32>),
    /// Already asked for (by anyone: Pulsarr asks for every Watchlist add) and not there yet.
    Requested,
    /// Seerr says it is all there.
    Have,
    /// Seerr's blocklist holds it.
    Blocked,
    /// A show with no episodes announced: nothing to ask for yet.
    NotYet,
}

/// The server's request settings that change what may be asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rules {
    /// A request may name some seasons; otherwise only every missing season at once.
    pub partial: bool,
    /// Season 0 (specials) counts as a season.
    pub specials: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detail {
    pub title: Title,
    pub minutes: u32,
    pub genres: Vec<String>,
    pub seasons: Vec<Season>,
    /// A request for the title that is still open (pending or approved), 4K requests aside.
    pub requested: bool,
    pub plex: Option<InPlex>,
    /// What the title's page shows besides the request state: credits, pictures, videos.
    pub info: Info,
}

/// A request is open until Seerr declines it or completes it (`DECLINED` 3, `COMPLETED` 5).
fn open_request(r: &Value) -> bool {
    !matches!(int(r, "status"), 3 | 5) && !r.get("is4k").and_then(Value::as_bool).unwrap_or(false)
}

fn season_numbers(r: &Value) -> impl Iterator<Item = u32> + '_ {
    r.get("seasons").and_then(Value::as_array).into_iter().flatten()
        .filter_map(|s| u32::try_from(int(s, "seasonNumber")).ok())
}

impl Detail {
    fn of(v: &Value, kind: Kind) -> Option<Self> {
        let title = Title::of(v, kind)?;
        let info = v.get("mediaInfo").filter(|i| i.is_object());
        let requests: Vec<&Value> = info.and_then(|i| i.get("requests")).and_then(Value::as_array)
            .map(|r| r.iter().filter(|r| open_request(r)).collect()).unwrap_or_default();
        let have_of = |number: u32| -> Have {
            info.and_then(|i| i.get("seasons")).and_then(Value::as_array).into_iter().flatten()
                .find(|s| int(s, "seasonNumber") == i64::from(number))
                .map_or(Have::Nothing, |s| Have::of(int(s, "status")))
        };
        let seasons = v.get("seasons").and_then(Value::as_array).into_iter().flatten()
            .filter_map(|s| {
                let number = u32::try_from(int(s, "seasonNumber")).ok()?;
                Some(Season {
                    number,
                    episodes: u32::try_from(int(s, "episodeCount")).unwrap_or(0),
                    have: have_of(number),
                    requested: requests.iter().any(|r| season_numbers(r).any(|n| n == number)),
                })
            })
            .collect();
        let minutes = match kind {
            Kind::Movie => int(v, "runtime"),
            Kind::Tv => v.get("episodeRunTime").and_then(Value::as_array).and_then(|r| r.first()).and_then(Value::as_i64).unwrap_or(0),
        };
        let genres = v.get("genres").and_then(Value::as_array).into_iter().flatten()
            .map(|g| if g.is_string() { g.as_str().unwrap_or("").to_string() } else { text(g, "name") })
            .filter(|g| !g.is_empty())
            .collect();
        let plex = info.filter(|_| matches!(title.state.have, Have::All | Have::Partly)).and_then(|i| {
            let url = text(i, "mediaUrl");
            let machine = url.split("/server/").nth(1)?.split(['/', '?', '#']).next()?.to_string();
            let rating_key = text(i, "ratingKey");
            (!machine.is_empty() && !rating_key.is_empty()).then_some(InPlex { machine, rating_key })
        });
        Some(Self {
            title,
            minutes: u32::try_from(minutes).unwrap_or(0),
            genres,
            seasons,
            requested: !requests.is_empty(),
            plex,
            info: Info::of(v, kind),
        })
    }

    /// The seasons a request could name: numbered (season 0 only when specials count) and
    /// announced with episodes.
    fn announced(&self, rules: Rules) -> impl Iterator<Item = &Season> {
        self.seasons.iter().filter(move |s| (s.number > 0 || rules.specials) && s.episodes > 0)
    }

    /// What the page may offer, by Seerr's own duplicate rules.
    pub fn ask(&self, rules: Rules) -> Ask {
        if self.title.state.have == Have::Blocklisted {
            return Ask::Blocked;
        }
        match self.title.kind {
            Kind::Movie => match self.title.state.have {
                Have::All | Have::Partly => Ask::Have,
                Have::Pending | Have::Processing => Ask::Requested,
                _ if self.requested => Ask::Requested,
                _ => Ask::Movie,
            },
            Kind::Tv => {
                let announced: Vec<&Season> = self.announced(rules).collect();
                let free: Vec<u32> = announced.iter()
                    .filter(|s| s.have == Have::Nothing && !s.requested)
                    .map(|s| s.number)
                    .collect();
                if !free.is_empty() {
                    Ask::Seasons(free)
                } else if announced.is_empty() {
                    Ask::NotYet
                } else if announced.iter().any(|s| s.requested || matches!(s.have, Have::Pending | Have::Processing)) {
                    Ask::Requested
                } else {
                    Ask::Have
                }
            }
        }
    }
}

/// One of the viewer's requests as the Requests page shows it: the title and how the request stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asked {
    pub title: Title,
    pub badge: Option<Badge>,
}

/// One of the viewer's own requests, newest first. Seerr names the title by id only; the page
/// reads the title itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MyRequest {
    pub id: u32,
    pub kind: Kind,
    pub tmdb: u32,
    pub badge: Option<Badge>,
}

impl MyRequest {
    fn of(r: &Value) -> Option<Self> {
        let media = r.get("media")?;
        let kind = match text(r, "type").as_str() {
            "movie" => Kind::Movie,
            "tv" => Kind::Tv,
            _ => return None,
        };
        let is4k = r.get("is4k").and_then(Value::as_bool).unwrap_or(false);
        let badge = match int(r, "status") {
            3 => Some(Badge::Declined),
            4 => Some(Badge::Failed),
            status => MediaState::of(media, is4k).badge().or(match status {
                1 => Some(Badge::Requested),
                2 => Some(Badge::Approved),
                5 => Some(Badge::Available),
                _ => None,
            }),
        };
        Some(Self {
            id: u32::try_from(int(r, "id")).ok()?,
            kind,
            tmdb: u32::try_from(int(media, "tmdbId")).ok().filter(|id| *id > 0)?,
            badge,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Me {
    pub id: u32,
    pub name: String,
}

/// Why a call did not produce what was asked. The screens say each in plain words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeerrError {
    /// No address set.
    NotSet,
    Unreachable,
    TimedOut,
    /// It answered, but not like Seerr.
    NotSeerr,
    /// No Plex account is signed in on the TV to sign in to Seerr with.
    NoPlexAccount,
    /// Seerr would not sign this Plex account in (it has no Seerr user, or Plex sign-in is off).
    SignInRefused,
    /// Too many requests: wait a minute.
    RateLimited,
    /// Seerr refused this account the action: a permission, a spent quota, a blocklisted title.
    Forbidden,
    NotFound,
    /// Already asked for: by now, or since the page opened.
    AlreadyRequested,
    /// Seerr failed (a `5xx`).
    Refused,
}

// ---- the calls --------------------------------------------------------------------------------------

/// One run of calls against one Seerr as one Plex account. The session starts as `cookie` (empty
/// when there is none); a call signs in when there is none or Seerr stopped accepting it, and
/// [`Seerr::cookie`] hands back the session to keep.
pub struct Seerr<'a, T: Transport> {
    base: &'a str,
    plex_token: &'a str,
    cookie: RefCell<String>,
    http: &'a T,
}

impl<'a, T: Transport> Seerr<'a, T> {
    pub fn new(base: &'a str, cookie: &str, plex_token: &'a str, http: &'a T) -> Self {
        Self { base, plex_token, cookie: RefCell::new(cookie.to_string()), http }
    }

    /// Another run of calls under the session held now, for a call made beside this one.
    pub fn sibling(&self) -> Self {
        Self::new(self.base, &self.cookie.borrow(), self.plex_token, self.http)
    }

    /// The session after the calls made: a new one when Seerr signed in again, empty when it would not.
    pub fn cookie(&self) -> String {
        self.cookie.borrow().clone()
    }

    fn send(&self, request: &Request) -> Result<Reply, SeerrError> {
        if self.base.is_empty() {
            return Err(SeerrError::NotSet);
        }
        let cookie = self.cookie.borrow().clone();
        self.http.send(self.base, &cookie, request).map_err(|e| match e {
            TransportError::Unreachable => SeerrError::Unreachable,
            TransportError::TimedOut => SeerrError::TimedOut,
        })
    }

    fn checked(reply: Reply) -> Result<Reply, SeerrError> {
        match reply.status {
            200..=299 => Ok(reply),
            401 => Err(SeerrError::SignInRefused),
            403 => Err(SeerrError::Forbidden),
            404 => Err(SeerrError::NotFound),
            409 => Err(SeerrError::AlreadyRequested),
            429 => Err(SeerrError::RateLimited),
            500..=599 => Err(SeerrError::Refused),
            _ => Err(SeerrError::NotSeerr),
        }
    }

    fn sign_in(&self) -> Result<(), SeerrError> {
        self.cookie.borrow_mut().clear();
        if self.plex_token.is_empty() {
            return Err(SeerrError::NoPlexAccount);
        }
        let body = json!({ "authToken": self.plex_token }).to_string();
        let reply = self.send(&Request { verb: Verb::Post, path: "/api/v1/auth/plex".into(), json: Some(body) })?;
        let reply = match reply.status {
            401 | 403 => return Err(SeerrError::SignInRefused),
            404 => return Err(SeerrError::NotSeerr),
            _ => Self::checked(reply)?,
        };
        let session = reply.cookies.iter().find(|c| c.starts_with("connect.sid=")).or(reply.cookies.first());
        let session = session.ok_or(SeerrError::NotSeerr)?;
        *self.cookie.borrow_mut() = session.clone();
        Ok(())
    }

    /// A call under the session: signed in first when there is none, and once more when Seerr no
    /// longer accepts the one held.
    fn call(&self, request: Request) -> Result<Reply, SeerrError> {
        if self.cookie.borrow().is_empty() {
            self.sign_in()?;
        }
        let mut reply = self.send(&request)?;
        if reply.status == 401 {
            self.sign_in()?;
            reply = self.send(&request)?;
        }
        Self::checked(reply)
    }

    fn get(&self, path: String) -> Result<Value, SeerrError> {
        let reply = self.call(Request { verb: Verb::Get, path, json: None })?;
        serde_json::from_slice(&reply.body).map_err(|_| SeerrError::NotSeerr)
    }

    /// `GET /api/v1/status`: is there a Seerr at this address? Needs no session.
    pub fn status(&self) -> Result<Status, SeerrError> {
        let reply = Self::checked(self.send(&Request { verb: Verb::Get, path: "/api/v1/status".into(), json: None })?)?;
        let v: Value = serde_json::from_slice(&reply.body).map_err(|_| SeerrError::NotSeerr)?;
        let version = text(&v, "version");
        if version.is_empty() {
            return Err(SeerrError::NotSeerr);
        }
        Ok(Status { version })
    }

    /// The Seerr user the Plex account signs in as.
    pub fn me(&self) -> Result<Me, SeerrError> {
        let v = self.get("/api/v1/auth/me".into())?;
        let id = u32::try_from(int(&v, "id")).ok().filter(|id| *id > 0).ok_or(SeerrError::NotSeerr)?;
        Ok(Me { id, name: text(&v, "displayName") })
    }

    pub fn rules(&self) -> Result<Rules, SeerrError> {
        let v = self.get("/api/v1/settings/public".into())?;
        let flag = |key: &str| v.get(key).and_then(Value::as_bool).unwrap_or(false);
        Ok(Rules { partial: flag("partialRequestsEnabled"), specials: flag("enableSpecialEpisodes") })
    }

    fn titles(&self, path: String) -> Result<Page<Title>, SeerrError> {
        let v = self.get(path)?;
        let results = v.get("results").and_then(Value::as_array).ok_or(SeerrError::NotSeerr)?;
        Ok(Page {
            items: results.iter().filter_map(Title::of_result).collect(),
            page: u32::try_from(int(&v, "page")).unwrap_or(1),
            pages: u32::try_from(int(&v, "totalPages")).unwrap_or(1),
        })
    }

    /// One page (from 1) of a discover shelf.
    pub fn discover(&self, shelf: Shelf, page: u32) -> Result<Page<Title>, SeerrError> {
        self.titles(format!("{}?page={page}", shelf.path()))
    }

    /// One page (from 1) of movies under `filter`, in `sort` order; see [`Browse::path`].
    pub fn browse(&self, filter: &Browse, sort: Sort, page: u32, today: &str) -> Result<Page<Title>, SeerrError> {
        self.titles(filter.path(sort, page, today))
    }

    /// The movie genres Seerr offers, with the ids `Browse::genre` takes.
    pub fn genres(&self) -> Result<Vec<Genre>, SeerrError> {
        let v = self.get("/api/v1/genres/movie".into())?;
        let list = v.as_array().ok_or(SeerrError::NotSeerr)?;
        Ok(list
            .iter()
            .filter_map(|g| {
                let id = u32::try_from(int(g, "id")).ok().filter(|id| *id > 0)?;
                let name = text(g, "name");
                (!name.is_empty()).then_some(Genre { id, name })
            })
            .collect())
    }

    /// One page (from 1) of a search for movies and shows.
    pub fn search(&self, query: &str, page: u32) -> Result<Page<Title>, SeerrError> {
        self.titles(format!("/api/v1/search?query={}&page={page}", enc(query.trim())))
    }

    pub fn detail(&self, kind: Kind, tmdb: u32) -> Result<Detail, SeerrError> {
        let v = self.get(format!("/api/v1/{}/{tmdb}", kind.word()))?;
        Detail::of(&v, kind).ok_or(SeerrError::NotSeerr)
    }

    /// The id Seerr gives the title's media record (`mediaInfo.id`), or `None` when it holds none.
    pub fn media_id(&self, kind: Kind, tmdb: u32) -> Result<Option<u32>, SeerrError> {
        match self.get(format!("/api/v1/{}/{tmdb}", kind.word())) {
            Ok(v) => Ok(v.get("mediaInfo").and_then(|m| u32::try_from(int(m, "id")).ok()).filter(|id| *id > 0)),
            Err(SeerrError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// What Seerr says of the title's requests and downloads, for the progress view. A title Seerr
    /// does not know has nothing in it.
    pub fn progress(&self, kind: Kind, tmdb: u32) -> Result<super::progress::SeerrView, SeerrError> {
        match self.get(format!("/api/v1/{}/{tmdb}", kind.word())) {
            Ok(v) => Ok(super::progress::SeerrView::of(&v)),
            Err(SeerrError::NotFound) => Ok(super::progress::SeerrView::default()),
            Err(error) => Err(error),
        }
    }

    /// Clear a media record and its requests (`DELETE /api/v1/media/{id}`, which needs the
    /// manage-requests permission). The title is not blocklisted: that would hide it from every user.
    /// A record already gone is done.
    pub fn delete_media(&self, id: u32) -> Result<(), SeerrError> {
        match self.call(Request { verb: Verb::Delete, path: format!("/api/v1/media/{id}"), json: None }) {
            Ok(_) | Err(SeerrError::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// The requests `user` made, newest first, [`REQUEST_PAGE`] from `skip`; `more` when Seerr holds
    /// further ones.
    pub fn my_requests(&self, user: u32, skip: u32) -> Result<(Vec<MyRequest>, bool), SeerrError> {
        let v = self.get(format!("/api/v1/request?take={REQUEST_PAGE}&skip={skip}&sort=added&requestedBy={user}"))?;
        let results = v.get("results").and_then(Value::as_array).ok_or(SeerrError::NotSeerr)?;
        let info = v.get("pageInfo");
        let more = info.is_some_and(|i| int(i, "page") < int(i, "pages"));
        Ok((results.iter().filter_map(MyRequest::of).collect(), more))
    }

    /// Ask for `kind` `tmdb` (and, for a show, these `seasons`). The title is read again first and
    /// nothing is sent unless [`Detail::ask`] still allows exactly this: a request Pulsarr (or anyone)
    /// made since the page read it answers [`SeerrError::AlreadyRequested`] without a duplicate.
    pub fn request(&self, kind: Kind, tmdb: u32, seasons: &[u32], rules: Rules) -> Result<(), SeerrError> {
        let now = self.detail(kind, tmdb)?;
        let body = match (now.ask(rules), kind) {
            (Ask::Movie, Kind::Movie) => json!({ "mediaType": "movie", "mediaId": tmdb }),
            (Ask::Seasons(free), Kind::Tv) if !seasons.is_empty() && seasons.iter().all(|s| free.contains(s)) => {
                json!({ "mediaType": "tv", "mediaId": tmdb, "seasons": seasons })
            }
            (Ask::Blocked, _) => return Err(SeerrError::Forbidden),
            _ => return Err(SeerrError::AlreadyRequested),
        };
        let reply = self.call(Request { verb: Verb::Post, path: "/api/v1/request".into(), json: Some(body.to_string()) })?;
        if reply.status == 202 {
            return Err(SeerrError::AlreadyRequested);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!("../../../../tests/fixtures/seerr/", $name))
        };
    }

    /// A fake Seerr: answers by exact path, only to the session it issued, and records every request.
    struct Fake {
        routes: Vec<(String, u16, &'static str)>,
        seen: RefCell<Vec<(String, Request)>>,
        signed_in: RefCell<u32>,
        refuse_sign_in: bool,
    }

    const SESSION: &str = "connect.sid=s%3Afresh";

    impl Fake {
        fn new(routes: &[(&str, u16, &'static str)]) -> Self {
            Self {
                routes: routes.iter().map(|(p, s, b)| (p.to_string(), *s, *b)).collect(),
                seen: RefCell::new(Vec::new()),
                signed_in: RefCell::new(0),
                refuse_sign_in: false,
            }
        }
        fn posts(&self) -> Vec<Request> {
            self.seen.borrow().iter().filter(|(_, r)| r.verb == Verb::Post && r.path == "/api/v1/request").map(|(_, r)| r.clone()).collect()
        }
    }

    impl Transport for Fake {
        fn send(&self, _base: &str, cookie: &str, request: &Request) -> Result<Reply, TransportError> {
            self.seen.borrow_mut().push((cookie.to_string(), request.clone()));
            let reply = |status, body: &str, cookies: Vec<String>| Ok(Reply { status, body: body.as_bytes().to_vec(), cookies });
            if request.path == "/api/v1/auth/plex" {
                if self.refuse_sign_in || request.json.as_deref() != Some(r#"{"authToken":"plex-token"}"#) {
                    return reply(401, "{}", Vec::new());
                }
                *self.signed_in.borrow_mut() += 1;
                return reply(200, fixture!("auth_me.json"), vec![SESSION.into(), "theme=dark".into()]);
            }
            if request.path != "/api/v1/status" && cookie != SESSION {
                return reply(401, "{}", Vec::new());
            }
            match self.routes.iter().find(|(p, _, _)| *p == request.path) {
                Some((_, status, body)) => reply(*status, body, Vec::new()),
                None => reply(404, "{}", Vec::new()),
            }
        }
    }

    const BASE: &str = "https://requests.example.net";
    const ALL_SEASONS: Rules = Rules { partial: true, specials: false };

    #[test]
    fn a_stale_session_signs_in_once_with_the_plex_token_and_the_new_cookie_is_handed_back() {
        let fake = Fake::new(&[("/api/v1/auth/me", 200, fixture!("auth_me.json"))]);
        let seerr = Seerr::new(BASE, "connect.sid=s%3Astale", "plex-token", &fake);
        assert_eq!(seerr.me(), Ok(Me { id: 1, name: "Viewer".into() }));
        assert_eq!(seerr.cookie(), SESSION, "the session cookie, not another cookie of the reply");
        assert_eq!(*fake.signed_in.borrow(), 1);
        assert_eq!(seerr.me().map(|m| m.id), Ok(1));
        assert_eq!(*fake.signed_in.borrow(), 1, "a held session is reused");
    }

    #[test]
    fn sign_in_failures_say_why() {
        let mut fake = Fake::new(&[("/api/v1/status", 200, fixture!("status.json"))]);
        assert_eq!(Seerr::new(BASE, "", "", &fake).me(), Err(SeerrError::NoPlexAccount));
        fake.refuse_sign_in = true;
        let seerr = Seerr::new(BASE, "", "plex-token", &fake);
        assert_eq!(seerr.me(), Err(SeerrError::SignInRefused));
        assert_eq!(seerr.cookie(), "");
        assert_eq!(seerr.status(), Ok(Status { version: "3.5.2".into() }), "status needs no session");
        assert_eq!(Seerr::new("", "", "plex-token", &fake).status(), Err(SeerrError::NotSet));
    }

    #[test]
    fn a_media_record_is_found_by_its_title_and_cleared_by_its_own_id() {
        let fake = Fake::new(&[
            ("/api/v1/movie/603", 200, r#"{"id":603,"mediaInfo":{"id":14,"status":5}}"#),
            ("/api/v1/tv/9", 200, r#"{"id":9}"#),
            ("/api/v1/media/14", 204, ""),
            ("/api/v1/media/15", 403, "{}"),
        ]);
        let seerr = Seerr::new(BASE, SESSION, "plex-token", &fake);
        assert_eq!(seerr.media_id(Kind::Movie, 603), Ok(Some(14)));
        assert_eq!(seerr.media_id(Kind::Tv, 9), Ok(None), "Seerr knows the title but holds no media for it");
        assert_eq!(seerr.media_id(Kind::Movie, 1), Ok(None), "an unknown title is nothing to clear");
        assert_eq!(seerr.delete_media(14), Ok(()));
        assert_eq!(seerr.delete_media(99), Ok(()), "already gone");
        assert_eq!(seerr.delete_media(15), Err(SeerrError::Forbidden));
        assert!(fake.seen.borrow().iter().any(|(_, r)| r.verb == Verb::Delete && r.path == "/api/v1/media/14"));
    }

    #[test]
    fn errors_map_to_plain_reasons() {
        for (status, error) in [(429, SeerrError::RateLimited), (403, SeerrError::Forbidden), (500, SeerrError::Refused), (302, SeerrError::NotSeerr)] {
            let fake = Fake::new(&[("/api/v1/discover/trending?page=1", status, "{}")]);
            assert_eq!(Seerr::new(BASE, SESSION, "plex-token", &fake).discover(Shelf::Trending, 1), Err(error), "{status}");
        }
        let html = Fake::new(&[("/api/v1/status", 200, "<html></html>")]);
        assert_eq!(Seerr::new(BASE, "", "", &html).status(), Err(SeerrError::NotSeerr));
    }

    #[test]
    fn discover_and_search_read_titles_badges_and_paging_and_drop_people() {
        let fake = Fake::new(&[
            ("/api/v1/discover/trending?page=1", 200, fixture!("discover_trending.json")),
            ("/api/v1/search?query=dune%20part&page=2", 200, fixture!("search_dune.json")),
        ]);
        let seerr = Seerr::new(BASE, SESSION, "plex-token", &fake);
        let trending = seerr.discover(Shelf::Trending, 1).unwrap();
        assert_eq!((trending.items.len(), trending.page, trending.pages, trending.more()), (10, 1, 500, true));
        let odyssey = &trending.items[0];
        assert_eq!((odyssey.kind, odyssey.tmdb, odyssey.name.as_str(), odyssey.year.as_str()), (Kind::Movie, 1368337, "The Odyssey", "2026"));
        assert_eq!(odyssey.state.badge(), Some(Badge::Available));
        assert_eq!(trending.items[1].state.badge(), None, "a title Seerr does not know yet has no badge");
        let godzilla = trending.items.iter().find(|t| t.tmdb == 1381071).unwrap();
        assert_eq!(godzilla.state.badge(), Some(Badge::Approved));
        assert_eq!(trending.items[4].kind, Kind::Tv);
        let found = seerr.search(" dune part ", 2).unwrap();
        assert!(!found.items.is_empty() && found.items.len() < 20, "people are not titles");
        assert!(found.items.iter().all(|t| !t.name.is_empty()));
    }

    #[test]
    fn a_title_already_requested_or_held_offers_no_request() {
        let movie = |body| Detail::of(&serde_json::from_str::<Value>(body).unwrap(), Kind::Movie).unwrap();
        let tv = |body| Detail::of(&serde_json::from_str::<Value>(body).unwrap(), Kind::Tv).unwrap();
        assert_eq!(movie(fixture!("movie_new.json")).ask(ALL_SEASONS), Ask::Movie);
        assert_eq!(movie(fixture!("movie_approved.json")).ask(ALL_SEASONS), Ask::Requested);
        let available = movie(fixture!("movie_available.json"));
        assert_eq!(available.ask(ALL_SEASONS), Ask::Have);
        assert_eq!(available.plex, Some(InPlex { machine: "0123456789abcdef0123456789abcdef01234567".into(), rating_key: "22192".into() }));
        assert_eq!(tv(fixture!("tv_new.json")).ask(ALL_SEASONS), Ask::Seasons(vec![1]), "season 0 has no episodes");
        let approved = tv(fixture!("tv_approved.json"));
        assert_eq!(approved.ask(ALL_SEASONS), Ask::Requested);
        assert!(approved.seasons.iter().any(|s| s.number == 1 && s.requested));
        let held = tv(fixture!("tv_available.json"));
        assert_eq!(held.ask(ALL_SEASONS), Ask::Have, "season 2 is announced without episodes; specials are off");
        assert_eq!(held.ask(Rules { partial: true, specials: true }), Ask::Seasons(vec![0]));
        assert_eq!(held.plex.as_ref().map(|p| p.rating_key.as_str()), Some("21977"));
    }

    #[test]
    fn a_request_reads_the_title_again_and_never_sends_a_duplicate() {
        let fake = Fake::new(&[
            ("/api/v1/movie/1381071", 200, fixture!("movie_approved.json")),
            ("/api/v1/movie/1300968", 200, fixture!("movie_new.json")),
            ("/api/v1/tv/291507", 200, fixture!("tv_new.json")),
            ("/api/v1/request", 201, "{}"),
        ]);
        let seerr = Seerr::new(BASE, SESSION, "plex-token", &fake);
        assert_eq!(seerr.request(Kind::Movie, 1381071, &[], ALL_SEASONS), Err(SeerrError::AlreadyRequested));
        assert_eq!(seerr.request(Kind::Tv, 291507, &[2], ALL_SEASONS), Err(SeerrError::AlreadyRequested), "a season not open");
        assert!(fake.posts().is_empty(), "nothing was sent for either");
        assert_eq!(seerr.request(Kind::Movie, 1300968, &[], ALL_SEASONS), Ok(()));
        assert_eq!(seerr.request(Kind::Tv, 291507, &[1], ALL_SEASONS), Ok(()));
        let bodies: Vec<_> = fake.posts().into_iter().filter_map(|r| r.json).collect();
        assert_eq!(bodies, [r#"{"mediaId":1300968,"mediaType":"movie"}"#, r#"{"mediaId":291507,"mediaType":"tv","seasons":[1]}"#]);
    }

    #[test]
    fn a_request_seerr_finds_nothing_left_in_is_already_requested() {
        let fake = Fake::new(&[("/api/v1/movie/1300968", 200, fixture!("movie_new.json")), ("/api/v1/request", 202, "{}")]);
        let seerr = Seerr::new(BASE, SESSION, "plex-token", &fake);
        assert_eq!(seerr.request(Kind::Movie, 1300968, &[], ALL_SEASONS), Err(SeerrError::AlreadyRequested));
        let fake = Fake::new(&[("/api/v1/movie/1300968", 200, fixture!("movie_new.json")), ("/api/v1/request", 409, "{}")]);
        assert_eq!(Seerr::new(BASE, SESSION, "plex-token", &fake).request(Kind::Movie, 1300968, &[], ALL_SEASONS), Err(SeerrError::AlreadyRequested));
    }

    #[test]
    fn my_requests_read_ids_badges_and_whether_more_wait() {
        let fake = Fake::new(&[
            ("/api/v1/request?take=20&skip=0&sort=added&requestedBy=1", 200, fixture!("requests_mine.json")),
            ("/api/v1/settings/public", 200, fixture!("settings_public.json")),
        ]);
        let seerr = Seerr::new(BASE, SESSION, "plex-token", &fake);
        let (mine, more) = seerr.my_requests(1, 0).unwrap();
        assert!(more);
        assert_eq!(mine.len(), 8);
        assert_eq!(mine[0], MyRequest { id: 201, kind: Kind::Movie, tmdb: 656908, badge: Some(Badge::Approved) });
        assert!(mine.iter().all(|r| r.badge.is_some()));
        assert_eq!(seerr.rules(), Ok(Rules { partial: true, specials: false }));
    }

    const TODAY: &str = "2026-08-15";

    #[test]
    fn a_filtered_listing_sends_genre_keyword_and_inclusive_year_bounds_to_seerr() {
        let all = Browse { genre: Some(28), keyword: Some(779), from: Some(1990), to: Some(1999) };
        assert_eq!(
            all.path(Sort::Popular, 2, TODAY),
            "/api/v1/discover/movies?page=2&sortBy=popularity.desc&genre=28&keywords=779&primaryReleaseDateGte=1990-01-01&primaryReleaseDateLte=1999-12-31"
        );
        assert!(all.path(Sort::TopRated, 1, TODAY).ends_with("&voteCountGte=100"));
        assert!(all.path(Sort::TopRated, 1, TODAY).contains("sortBy=vote_average.desc"));
        let one_year = Browse { from: Some(2005), to: Some(2005), ..Browse::default() };
        assert!(one_year.path(Sort::Popular, 1, TODAY).ends_with("&primaryReleaseDateGte=2005-01-01&primaryReleaseDateLte=2005-12-31"));
        assert_eq!(Browse::default().path(Sort::Popular, 1, TODAY), "/api/v1/discover/movies?page=1&sortBy=popularity.desc");
    }

    #[test]
    fn a_range_with_one_end_is_closed_and_newest_never_lists_the_unreleased() {
        let from = Browse { from: Some(2010), ..Browse::default() };
        assert!(from.path(Sort::Popular, 1, TODAY).ends_with("&primaryReleaseDateGte=2010-01-01&primaryReleaseDateLte=2026-08-15"));
        let to = Browse { to: Some(1979), ..Browse::default() };
        assert!(to.path(Sort::Popular, 1, TODAY).ends_with("&primaryReleaseDateGte=1900-01-01&primaryReleaseDateLte=1979-12-31"));
        let newest = Browse::default().path(Sort::Newest, 1, TODAY);
        assert_eq!(
            newest,
            "/api/v1/discover/movies?page=1&sortBy=primary_release_date.desc&primaryReleaseDateLte=2026-08-15&voteCountGte=5"
        );
        assert!(to.path(Sort::Newest, 1, TODAY).contains("&primaryReleaseDateLte=1979-12-31&"), "an earlier end wins over today");
        let future = Browse { to: Some(2030), ..Browse::default() };
        assert!(future.path(Sort::Newest, 1, TODAY).contains("&primaryReleaseDateLte=2026-08-15&"), "a later end is cut at today");
    }

    #[test]
    fn a_filtered_listing_reads_titles_and_paging_and_an_empty_answer_is_a_page_with_no_titles() {
        let found = r#"{"page":1,"totalPages":14,"totalResults":272,"keywords":[],"results":[
            {"id":603,"mediaType":"movie","title":"The Matrix","releaseDate":"1999-03-30","posterPath":"/m.jpg","mediaInfo":{"status":5,"tmdbId":603}},
            {"id":8,"mediaType":"movie","title":"Another","releaseDate":"1998-01-01","posterPath":"/a.jpg"}]}"#;
        let none = r#"{"page":1,"totalPages":0,"totalResults":0,"keywords":[],"results":[]}"#;
        let hit = Browse { genre: Some(28), keyword: Some(779), from: Some(1990), to: Some(1999) };
        let miss = Browse { genre: Some(99), keyword: Some(12339), from: Some(1930), to: Some(1939) };
        let fake = Fake::new(&[
            ("/api/v1/discover/movies?page=1&sortBy=popularity.desc&genre=28&keywords=779&primaryReleaseDateGte=1990-01-01&primaryReleaseDateLte=1999-12-31", 200, found),
            ("/api/v1/discover/movies?page=1&sortBy=popularity.desc&genre=99&keywords=12339&primaryReleaseDateGte=1930-01-01&primaryReleaseDateLte=1939-12-31", 200, none),
        ]);
        let seerr = Seerr::new(BASE, SESSION, "plex-token", &fake);
        let page = seerr.browse(&hit, Sort::Popular, 1, TODAY).unwrap();
        assert_eq!((page.items.len(), page.page, page.pages, page.more()), (2, 1, 14, true));
        assert_eq!((page.items[0].tmdb, page.items[0].year.as_str()), (603, "1999"));
        let empty = seerr.browse(&miss, Sort::Popular, 1, TODAY).unwrap();
        assert_eq!((empty.items.len(), empty.more()), (0, false));
    }

    #[test]
    fn genres_are_read_as_ids_and_names_and_an_answer_that_is_not_a_list_is_not_seerr() {
        let fake = Fake::new(&[(
            "/api/v1/genres/movie",
            200,
            r#"[{"id":28,"name":"Action"},{"id":878,"name":"Science Fiction"},{"id":0,"name":"Bad"},{"id":5,"name":""}]"#,
        )]);
        let seerr = Seerr::new(BASE, SESSION, "plex-token", &fake);
        assert_eq!(seerr.genres(), Ok(vec![Genre { id: 28, name: "Action".into() }, Genre { id: 878, name: "Science Fiction".into() }]));
        let object = Fake::new(&[("/api/v1/genres/movie", 200, "{}")]);
        assert_eq!(Seerr::new(BASE, SESSION, "plex-token", &object).genres(), Err(SeerrError::NotSeerr));
    }

    #[test]
    fn images_come_from_tmdb_at_a_published_width() {
        assert_eq!(tmdb_image("/a.jpg", "w342"), "https://image.tmdb.org/t/p/w342/a.jpg");
        assert_eq!(tmdb_image("", "w342"), "");
    }
}
