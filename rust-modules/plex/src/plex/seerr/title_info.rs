//! What a title's page shows beyond the request state: the words, credits, pictures and videos
//! that Seerr's `GET /api/v1/{movie|tv}/{id}` carries (TMDB's own fields, passed through), read
//! from the same answer as [`Detail`](super::Detail), plus the episode stills of
//! `GET /api/v1/tv/{id}/season/{n}`.
//!
//! Seerr has no image gallery for a movie, so a film's gallery is its videos' own thumbnails;
//! a series' gallery is its episode stills.
use super::{int, text, tmdb_image, Kind, Seerr, SeerrError, Transport};
use serde_json::Value;

/// Writers and studios named in the facts: the page has room for a line, not a credits roll.
const NAMED: usize = 3;

/// Where a title stands in its life, from TMDB's `status` word (which is English only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Released,
    Returning,
    Ended,
    Canceled,
    InProduction,
    Planned,
}

impl Stage {
    fn of(word: &str) -> Option<Self> {
        Some(match word {
            "Released" => Stage::Released,
            "Returning Series" => Stage::Returning,
            "Ended" => Stage::Ended,
            "Canceled" => Stage::Canceled,
            "In Production" | "Post Production" | "Pilot" => Stage::InProduction,
            "Planned" | "Rumored" => Stage::Planned,
            _ => return None,
        })
    }
}

/// What a person on the cast shelf did on the title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    Director,
    Creator,
    /// The part played.
    Character(String),
}

/// A person with a headshot: the page's cast shelf lists only people TMDB has a picture of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub id: u32,
    pub name: String,
    pub role: Role,
    /// The headshot's whole address on TMDB.
    pub photo: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoKind {
    Trailer,
    Teaser,
    Clip,
    Featurette,
    Other,
}

impl VideoKind {
    fn of(word: &str) -> Self {
        match word {
            "Trailer" => VideoKind::Trailer,
            "Teaser" => VideoKind::Teaser,
            "Clip" => VideoKind::Clip,
            "Featurette" => VideoKind::Featurette,
            _ => VideoKind::Other,
        }
    }
    /// Trailers first on the shelf, then the shorter promos, then the rest.
    fn rank(self) -> u8 {
        match self {
            VideoKind::Trailer => 0,
            VideoKind::Teaser => 1,
            VideoKind::Clip => 2,
            VideoKind::Featurette => 3,
            VideoKind::Other => 4,
        }
    }
}

/// One YouTube video of a title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Video {
    /// YouTube's video id: only letters, digits, `-` and `_`.
    pub key: String,
    pub name: String,
    pub kind: VideoKind,
}

impl Video {
    /// The video's own still on YouTube, 480x360.
    pub fn thumb(&self) -> String {
        format!("https://img.youtube.com/vi/{}/hqdefault.jpg", self.key)
    }
}

/// A YouTube video id is 11 characters of `[A-Za-z0-9_-]`; the key is spliced into a launch
/// request, so anything else is refused.
pub fn is_video_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 32 && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A season's poster on TMDB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeasonArt {
    pub number: u32,
    pub poster: String,
}

/// One episode still of a series.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Still {
    pub season: u32,
    pub episode: u32,
    pub name: String,
    pub image: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Info {
    pub tagline: String,
    /// The title in its own language; empty when it is the name shown.
    pub original_name: String,
    /// The US certification (`PG-13`, `TV-MA`), empty when there is none.
    pub certification: String,
    /// TMDB's audience score out of 100; 0 while nobody has voted.
    pub score: u32,
    pub stage: Option<Stage>,
    pub directors: Vec<String>,
    pub writers: Vec<String>,
    pub creators: Vec<String>,
    pub studios: Vec<String>,
    pub networks: Vec<String>,
    /// US dollars; 0 when TMDB does not know.
    pub budget: u64,
    pub revenue: u64,
    pub seasons: u32,
    pub episodes: u32,
    /// Directors or creators first, then the cast in billing order; only people with a picture.
    pub people: Vec<Person>,
    pub videos: Vec<Video>,
    pub season_art: Vec<SeasonArt>,
}

fn list<'a>(v: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> + 'a {
    v.get(key).and_then(Value::as_array).into_iter().flatten()
}

fn name_of(v: &Value) -> String {
    text(v, "name").trim().to_string()
}

fn push_name(into: &mut Vec<String>, name: String) {
    if !name.is_empty() && !into.contains(&name) {
        into.push(name);
    }
}

fn person(v: &Value, role: Role) -> Option<Person> {
    let photo = tmdb_image(&text(v, "profilePath"), "w342");
    let name = name_of(v);
    (!photo.is_empty() && !name.is_empty()).then(|| Person { id: u32::try_from(int(v, "id")).unwrap_or(0), name, role, photo })
}

fn certification(v: &Value, kind: Kind) -> String {
    match kind {
        Kind::Movie => list(&v["releases"], "results")
            .find(|r| text(r, "iso_3166_1") == "US")
            .into_iter()
            .flat_map(|r| list(r, "release_dates"))
            .map(|d| text(d, "certification"))
            .find(|c| !c.is_empty())
            .unwrap_or_default(),
        Kind::Tv => list(&v["contentRatings"], "results")
            .find(|r| text(r, "iso_3166_1") == "US")
            .map(|r| text(r, "rating"))
            .unwrap_or_default(),
    }
}

impl Info {
    pub(super) fn of(v: &Value, kind: Kind) -> Self {
        let crew = || list(&v["credits"], "crew");
        let mut directors = Vec::new();
        let mut writers = Vec::new();
        let mut creators = Vec::new();
        let mut people: Vec<Person> = Vec::new();
        for member in crew() {
            let job = text(member, "job");
            match job.as_str() {
                "Director" => {
                    push_name(&mut directors, name_of(member));
                    people.extend(person(member, Role::Director));
                }
                "Writer" | "Screenplay" | "Story" | "Teleplay" => push_name(&mut writers, name_of(member)),
                _ => {}
            }
        }
        for maker in list(v, "createdBy") {
            push_name(&mut creators, name_of(maker));
            people.extend(person(maker, Role::Creator));
        }
        for member in list(&v["credits"], "cast") {
            people.extend(person(member, Role::Character(text(member, "character").trim().to_string())));
        }
        // A director who also acts is on the shelf once, in the lead.
        let mut seen = Vec::new();
        people.retain(|p| p.id == 0 || {
            let fresh = !seen.contains(&p.id);
            seen.push(p.id);
            fresh
        });
        directors.truncate(NAMED);
        writers.truncate(NAMED);

        let mut videos: Vec<Video> = list(v, "relatedVideos")
            .filter(|r| text(r, "site") == "YouTube")
            .filter_map(|r| {
                let key = text(r, "key");
                is_video_key(&key).then(|| Video { key, name: name_of(r), kind: VideoKind::of(&text(r, "type")) })
            })
            .collect();
        videos.sort_by_key(|video| video.kind.rank());

        let names = |key: &str| -> Vec<String> {
            let mut out = Vec::new();
            for entry in list(v, key) {
                push_name(&mut out, name_of(entry));
            }
            out.truncate(NAMED);
            out
        };
        let original = match kind {
            Kind::Movie => text(v, "originalTitle"),
            Kind::Tv => text(v, "originalName"),
        };
        let shown = match kind {
            Kind::Movie => text(v, "title"),
            Kind::Tv => text(v, "name"),
        };
        let votes = int(v, "voteCount");
        let average = v.get("voteAverage").and_then(Value::as_f64).unwrap_or(0.0);
        Self {
            tagline: text(v, "tagline").trim().to_string(),
            original_name: if original == shown { String::new() } else { original.trim().to_string() },
            certification: certification(v, kind),
            score: if votes > 0 { (average * 10.0).round().clamp(0.0, 100.0) as u32 } else { 0 },
            stage: Stage::of(&text(v, "status")),
            directors,
            writers,
            creators,
            studios: names("productionCompanies"),
            networks: names("networks"),
            budget: u64::try_from(int(v, "budget")).unwrap_or(0),
            revenue: u64::try_from(int(v, "revenue")).unwrap_or(0),
            seasons: u32::try_from(int(v, "numberOfSeasons")).unwrap_or(0),
            episodes: u32::try_from(int(v, "numberOfEpisodes")).unwrap_or(0),
            people,
            videos,
            season_art: list(v, "seasons")
                .filter_map(|s| {
                    let poster = tmdb_image(&text(s, "posterPath"), "w342");
                    Some(SeasonArt { number: u32::try_from(int(s, "seasonNumber")).ok()?, poster }).filter(|s| !s.poster.is_empty())
                })
                .collect(),
        }
    }

    /// The video the Trailer action opens: the first official trailer Seerr lists.
    pub fn trailer(&self) -> Option<&Video> {
        self.videos.iter().find(|video| video.kind == VideoKind::Trailer)
    }

    pub fn season_poster(&self, number: u32) -> Option<&str> {
        self.season_art.iter().find(|s| s.number == number).map(|s| s.poster.as_str())
    }
}

/// The stills of one season's episodes, in air order; an episode TMDB has no still for is left out.
fn stills_of(v: &Value, season: u32) -> Vec<Still> {
    list(v, "episodes")
        .filter_map(|e| {
            let image = tmdb_image(&text(e, "stillPath"), "w780");
            (!image.is_empty()).then(|| Still {
                season,
                episode: u32::try_from(int(e, "episodeNumber")).unwrap_or(0),
                name: name_of(e),
                image,
            })
        })
        .collect()
}

impl<T: Transport> Seerr<'_, T> {
    /// `GET /api/v1/tv/{id}/season/{n}`: the season's episodes that have a still.
    pub fn stills(&self, tmdb: u32, season: u32) -> Result<Vec<Still>, SeerrError> {
        let v = self.get(format!("/api/v1/tv/{tmdb}/season/{season}"))?;
        Ok(stills_of(&v, season))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn movie() -> Value {
        json!({
            "id": 603, "title": "The Matrix", "originalTitle": "The Matrix", "tagline": "Welcome to the Real World.",
            "status": "Released", "voteAverage": 8.2, "voteCount": 25000, "budget": 63000000, "revenue": 463517383,
            "productionCompanies": [{"id": 79, "name": "Village Roadshow Pictures"}, {"id": 174, "name": "Warner Bros. Pictures"}],
            "releases": {"results": [
                {"iso_3166_1": "DE", "release_dates": [{"certification": "16"}]},
                {"iso_3166_1": "US", "release_dates": [{"certification": ""}, {"certification": "R"}]}
            ]},
            "credits": {
                "cast": [
                    {"id": 6384, "name": "Keanu Reeves", "character": "Thomas A. Anderson / Neo", "profilePath": "/rRdru6REr9i3WIHv2mntpcgxnoY.jpg", "order": 0},
                    {"id": 2975, "name": "Laurence Fishburne", "character": "Morpheus", "profilePath": "/abc.jpg", "order": 1},
                    {"id": 9999, "name": "No Photo", "character": "Extra", "profilePath": null, "order": 2}
                ],
                "crew": [
                    {"id": 9340, "name": "Lana Wachowski", "job": "Director", "profilePath": "/lana.jpg"},
                    {"id": 9341, "name": "Lilly Wachowski", "job": "Director", "profilePath": "/lilly.jpg"},
                    {"id": 9340, "name": "Lana Wachowski", "job": "Writer", "profilePath": "/lana.jpg"},
                    {"id": 1, "name": "Joel Silver", "job": "Producer", "profilePath": "/joel.jpg"}
                ]
            },
            "relatedVideos": [
                {"site": "YouTube", "key": "clip11chars", "name": "Opening scene", "type": "Clip"},
                {"site": "YouTube", "key": "m8e-h2R9pZU", "name": "Official Trailer", "type": "Trailer"},
                {"site": "Vimeo", "key": "123", "name": "Not YouTube", "type": "Trailer"},
                {"site": "YouTube", "key": "bad key&x=1", "name": "Bad", "type": "Trailer"}
            ]
        })
    }

    fn series() -> Value {
        json!({
            "id": 1396, "name": "Breaking Bad", "originalName": "Breaking Bad", "status": "Ended", "voteAverage": 8.9, "voteCount": 14000,
            "numberOfSeasons": 5, "numberOfEpisodes": 62,
            "createdBy": [{"id": 66633, "name": "Vince Gilligan", "profilePath": "/vince.jpg"}],
            "networks": [{"id": 174, "name": "AMC"}],
            "contentRatings": {"results": [{"iso_3166_1": "US", "rating": "TV-MA"}]},
            "credits": {"cast": [{"id": 17419, "name": "Bryan Cranston", "character": "Walter White", "profilePath": "/bryan.jpg"}], "crew": []},
            "seasons": [{"seasonNumber": 0, "posterPath": null}, {"seasonNumber": 1, "posterPath": "/s1.jpg"}],
            "relatedVideos": []
        })
    }

    #[test]
    fn a_film_reads_its_credits_facts_and_certification() {
        let info = Info::of(&movie(), Kind::Movie);
        assert_eq!(info.tagline, "Welcome to the Real World.");
        assert_eq!((info.certification.as_str(), info.score), ("R", 82));
        assert_eq!(info.stage, Some(Stage::Released));
        assert_eq!(info.directors, ["Lana Wachowski", "Lilly Wachowski"]);
        assert_eq!(info.writers, ["Lana Wachowski"]);
        assert_eq!(info.studios, ["Village Roadshow Pictures", "Warner Bros. Pictures"]);
        assert_eq!((info.budget, info.revenue), (63_000_000, 463_517_383));
        assert!(info.original_name.is_empty(), "the original name is only kept when it differs");
    }

    #[test]
    fn the_cast_shelf_lists_directors_first_and_only_people_with_a_picture() {
        let info = Info::of(&movie(), Kind::Movie);
        let names: Vec<&str> = info.people.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Lana Wachowski", "Lilly Wachowski", "Keanu Reeves", "Laurence Fishburne"]);
        assert_eq!(info.people[0].role, Role::Director);
        assert_eq!(info.people[2].role, Role::Character("Thomas A. Anderson / Neo".into()));
        assert_eq!(info.people[2].photo, "https://image.tmdb.org/t/p/w342/rRdru6REr9i3WIHv2mntpcgxnoY.jpg");
    }

    #[test]
    fn credits_and_videos_keep_entries_past_the_first_shelf_window() {
        let cast: Vec<_> = (0..25).map(|id| json!({
            "id": id, "name": "Cast member", "profilePath": "/cast.jpg"
        })).collect();
        let videos: Vec<_> = (0..25).map(|id| json!({
            "name": "Trailer", "site": "YouTube", "key": format!("{id:011}"), "type": "Trailer"
        })).collect();
        let details = json!({"credits": {"cast": cast}, "relatedVideos": videos});
        let info = Info::of(&details, Kind::Movie);
        assert_eq!(info.people.last().unwrap().id, 24);
        assert_eq!(info.videos.last().unwrap().key, "00000000024");
    }

    #[test]
    fn videos_are_youtube_only_with_a_clean_key_and_trailers_first() {
        let info = Info::of(&movie(), Kind::Movie);
        let keys: Vec<&str> = info.videos.iter().map(|v| v.key.as_str()).collect();
        assert_eq!(keys, ["m8e-h2R9pZU", "clip11chars"]);
        assert_eq!(info.trailer().map(|v| v.key.as_str()), Some("m8e-h2R9pZU"));
        assert_eq!(info.videos[0].thumb(), "https://img.youtube.com/vi/m8e-h2R9pZU/hqdefault.jpg");
        assert!(is_video_key("m8e-h2R9pZU") && !is_video_key("a b") && !is_video_key("") && !is_video_key("x?y=1"));
    }

    #[test]
    fn a_title_with_no_videos_has_no_trailer() {
        let info = Info::of(&series(), Kind::Tv);
        assert!(info.videos.is_empty() && info.trailer().is_none());
    }

    #[test]
    fn a_series_reads_its_makers_network_extent_and_season_posters() {
        let info = Info::of(&series(), Kind::Tv);
        assert_eq!(info.certification, "TV-MA");
        assert_eq!(info.creators, ["Vince Gilligan"]);
        assert_eq!(info.networks, ["AMC"]);
        assert_eq!((info.seasons, info.episodes, info.stage), (5, 62, Some(Stage::Ended)));
        assert_eq!(info.people[0].role, Role::Creator);
        assert_eq!(info.season_poster(1), Some("https://image.tmdb.org/t/p/w342/s1.jpg"));
        assert_eq!(info.season_poster(0), None, "a season without a poster has none to show");
    }

    #[test]
    fn an_unrated_or_bare_answer_reads_as_nothing_rather_than_zero_stars() {
        let info = Info::of(&json!({"id": 1, "title": "X", "voteAverage": 0.0, "voteCount": 0}), Kind::Movie);
        assert_eq!(info, Info::default());
    }

    #[test]
    fn stills_keep_only_episodes_that_have_one() {
        let season = json!({"episodes": [
            {"episodeNumber": 1, "name": "Pilot", "stillPath": "/e1.jpg"},
            {"episodeNumber": 2, "name": "No still", "stillPath": null},
            {"episodeNumber": 3, "name": "Third", "stillPath": "/e3.jpg"}
        ]});
        let stills = stills_of(&season, 1);
        assert_eq!(stills.iter().map(|s| s.episode).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(stills[0].image, "https://image.tmdb.org/t/p/w780/e1.jpg");
    }
}
