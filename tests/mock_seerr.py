"""The mock's Seerr + Radarr + Sonarr stand-in, for the Discover/Requests screenshots.

Answers on the same loopback listener as the mock PMS (`mock_pms.MockPms.handle` asks `SeerrFixture.handle`
first). Wire shapes are Seerr 3.5.2's (`tests/fixtures/seerr/*.json` are recorded answers; the generated
ones here carry exactly the fields `plex::seerr` reads) and Radarr/Sonarr v3's. The titles are the demo
library's (openly licensed, see CREDITS), given synthetic TMDB/TVDB ids; their artwork is the library's own
poster and art. The app asks the PMS photo proxy for `image.tmdb.org/t/p/<size>/demo/<slug>/...`,
and the mock answers with the demo artwork instead of fetching from the network.

Everything is pure and pinned: no clock, no randomness, so two runs answer byte-identically.
The app is pointed here by the scene's own setup (Seerr address + the API keys below); nothing in the
product knows this module exists.
"""
import json
import re
import urllib.parse

SESSION_COOKIE = "connect.sid=demo-seerr-session"
RADARR_KEY = "demo-radarr-key"
SONARR_KEY = "demo-sonarr-key"
PAGE = 20

# TMDB's own genre ids (the Seerr `genre=` filter and `/genres/*` lists use them).
GENRE_IDS = {"Action": 28, "Adventure": 12, "Animation": 16, "Comedy": 35, "Documentary": 99, "Drama": 18,
             "Fantasy": 14, "Horror": 27, "Music": 10402, "Musical": 10402, "Mystery": 9648, "Romance": 10749,
             "Science Fiction": 878}
GENRE_NAMES = {28: "Action", 12: "Adventure", 16: "Animation", 35: "Comedy", 99: "Documentary", 18: "Drama",
               14: "Fantasy", 27: "Horror", 10402: "Music", 9648: "Mystery", 10749: "Romance",
               878: "Science Fiction"}

# Mood tags (TMDB keyword ids) and the demo titles that carry them.
KEYWORDS = {
    4565: ("dystopia", ["Tears of Steel", "Elephants Dream", "Charge", "Metropolis"]),
    4379: ("time travel", ["A Trip to the Moon", "Tears of Steel", "Hero"]),
    10051: ("heist", ["Sherlock Jr.", "The Kid", "Safety Last!", "The General", "Wing It!"]),
    10123: ("dark comedy", ["Sherlock Jr.", "The Kid", "The General", "Sprite Fright"]),
    207268: ("neo-noir", ["Night of the Living Dead", "Plan 9 from Outer Space", "His Girl Friday", "Charge"]),
}

# Requests the demo account has made: title -> (request status, media status).
# Seerr media status: 1 unknown, 2 pending, 3 processing, 4 partial, 5 available.
MOVIE_STATE = {"Spring": 5, "Sintel": 5, "Tears of Steel": 3, "Big Buck Bunny": 5, "Coffee Run": 2, "Hero": 3}
TV_STATE = {"Caminandes": 3, "Hubblecast": 5}
REQUESTS = [("movie", "Spring", 5, 5), ("movie", "Hero", 2, 3), ("movie", "Coffee Run", 1, 2),
            ("tv", "Caminandes", 2, 3), ("movie", "Tears of Steel", 2, 3)]
# The downloading titles: (size, size left) in bytes and the time left; a queue record per title.
MOVIE_QUEUE = {"Tears of Steel": (4_800_000_000, 1_824_000_000, "00:17:42")}
TV_QUEUE = {"Caminandes": [((1, 1), 1_200_000_000, 480_000_000, "00:06:10"),
                           ((1, 2), 1_150_000_000, 1_150_000_000, "00:00:00")]}
UPCOMING_FROM = 2021

TEARS_OF_STEEL_PLOT = (
    "\n\nYears later, Amsterdam is under siege. Thom joins a small team of scientists and fighters "
    "who have turned a former church into a last-ditch research lab. They believe that one conversation "
    "from his past might change what happened, if they can reconstruct it closely enough. While machines "
    "close in, the team rebuilds the old meeting place and asks Thom to face the moment he walked away."
    "\n\nTheir experiment offers no clean reset. Thom must decide what to say to Celia without knowing "
    "whether she will hear him, or whether his friends can hold the lab long enough for it to matter. "
    "The chase through the city gives way to an unlikely second chance, built on the simple act of listening."
    "\n\nOutside, the ruined streets bear little resemblance to the city they remember. The team can "
    "only guess at how a single choice led from that quiet afternoon to a battle at the church doors. "
    "Inside the reconstruction, there is finally time for both of them to finish the conversation."
)


def _json(obj, status=200, headers=None):
    return (status, "application/json", json.dumps(obj).encode(), headers or {})


class SeerrFixture:
    def __init__(self, catalog, lib=None):
        """`catalog` is the parsed demo catalog.json; `lib` the CatalogLibrary (artwork + rating keys)."""
        self.lib = lib
        self.page_size = PAGE
        self.titles = []
        for kind, rows, base in (("movie", catalog["movies"], 700_000), ("tv", catalog["shows"], 800_000)):
            for n, row in enumerate(rows):
                row = dict(row, kind=kind, tmdb=base + n, tvdb=900_000 + n if kind == "tv" else None,
                           order=n, slug=row["id"], vote_count=250 - 3 * n,
                           vote_average=round(6.4 + (n * 7 % 20) / 10, 1))
                self.titles.append(row)
        self.by_key = {(t["kind"], t["tmdb"]): t for t in self.titles}
        self.by_title = {(t["kind"], t["title"]): t for t in self.titles}
        self.posted = []  # POST /api/v1/request bodies, for the harness

    # ---- artwork -----------------------------------------------------------------------------------
    TMDB_IMAGE = re.compile(r"https://image\.tmdb\.org/t/p/[^/]+/demo/([a-z0-9-]+)/(poster|backdrop|still/\d+/\d+)\.jpg")

    def image(self, url, width, height):
        """`(content type, bytes)` for a proxied TMDB URL naming a demo title; None otherwise."""
        m = self.TMDB_IMAGE.fullmatch(url)
        if not m or self.lib is None:
            return None
        slug, kind = m.groups()
        if kind.startswith("still/"):
            rk, role = self.lib.by_slug.get(f"{slug}/{kind.removeprefix('still/')}"), "thumb"
        else:
            rk, role = self.lib.by_slug.get(slug), "thumb" if kind == "poster" else "art"
        return self.lib.image(f"/library/metadata/{rk}/{role}", width, height) if rk else None

    def _rating_key(self, t):
        return self.lib.by_slug.get(t["slug"]) if self.lib is not None else None

    # ---- shapes --------------------------------------------------------------------------------------
    def _state(self, t):
        return (MOVIE_STATE if t["kind"] == "movie" else TV_STATE).get(t["title"])

    def _requests_of(self, t):
        out = []
        for n, (kind, title, rstatus, _media) in enumerate(REQUESTS):
            if (kind, title) == (t["kind"], t["title"]):
                seasons = [{"seasonNumber": 1, "status": rstatus}] if kind == "tv" else []
                out.append({"id": 190 + n, "status": rstatus, "type": kind, "is4k": False, "seasons": seasons})
        return out

    def _download_status(self, t):
        if t["kind"] == "movie" and t["title"] in MOVIE_QUEUE:
            size, left, eta = MOVIE_QUEUE[t["title"]]
            return [{"status": "downloading", "size": size, "sizeLeft": left, "timeLeft": eta, "title": t["title"]}]
        if t["kind"] == "tv" and t["title"] in TV_QUEUE:
            return [{"status": "downloading" if left < size else "queued", "size": size, "sizeLeft": left,
                     "timeLeft": eta, "episode": {"seasonNumber": s, "episodeNumber": e}}
                    for (s, e), size, left, eta in TV_QUEUE[t["title"]]]
        return []

    def _media_info(self, t, full=True):
        status = self._state(t)
        if status is None:
            return None
        info = {"status": status, "status4k": 1, "tmdbId": t["tmdb"], "tvdbId": t["tvdb"],
                "mediaType": t["kind"], "downloadStatus": self._download_status(t),
                "ratingKey": str(self._rating_key(t)) if status == 5 and self._rating_key(t) else None}
        if full:
            seasons = []
            if t["kind"] == "tv":
                seasons = [{"seasonNumber": s["index"], "status": 5 if status == 5 else 3} for s in t["seasons"]]
            info["seasons"] = seasons
            info["requests"] = self._requests_of(t)
        return info

    def _genre_objs(self, t):
        return [{"id": GENRE_IDS[g], "name": g} for g in t["genres"] if g in GENRE_IDS]

    def _keywords(self, t):
        return [k for k, (_n, titles) in KEYWORDS.items() if t["title"] in titles and t["kind"] == "movie"]

    def _row(self, t):
        date = t.get("originallyAvailableAt") or f"{t['year']}-01-01"
        row = {"id": t["tmdb"], "mediaType": t["kind"], "posterPath": f"/demo/{t['slug']}/poster.jpg",
               "backdropPath": f"/demo/{t['slug']}/backdrop.jpg", "overview": t["summary"],
               "genreIds": [g["id"] for g in self._genre_objs(t)], "voteAverage": t["vote_average"],
               "voteCount": t["vote_count"], "popularity": 100 - t["order"]}
        if t["kind"] == "movie":
            row.update(title=t["title"], releaseDate=date)
        else:
            row.update(name=t["title"], firstAirDate=f"{t['year']}-01-01")
        info = self._media_info(t, full=False)
        if info:
            row["mediaInfo"] = info
        return row

    def _page(self, rows, q):
        page = max(1, int(q.get("page", "1") or 1))
        pages = -(-len(rows) // self.page_size)
        chunk = rows[(page - 1) * self.page_size:page * self.page_size]
        return {"page": page, "totalPages": pages if rows else 0, "totalResults": len(rows),
                "results": [self._row(t) for t in chunk]}

    # ---- discover ------------------------------------------------------------------------------------
    def _filtered(self, kind, q):
        rows = [t for t in self.titles if t["kind"] == kind]
        if q.get("genre"):
            want = {int(g) for g in re.split(r"[,|]", q["genre"]) if g.isdigit()}
            rows = [t for t in rows if want <= {g["id"] for g in self._genre_objs(t)}]
        lo = q.get("primaryReleaseDateGte") or q.get("firstAirDateGte")
        hi = q.get("primaryReleaseDateLte") or q.get("firstAirDateLte")
        if lo:
            rows = [t for t in rows if f"{t['year']:04d}-12-31" >= lo]
        if hi:
            rows = [t for t in rows if f"{t['year']:04d}-01-01" <= hi]
        if q.get("keywords"):
            want = {int(k) for k in re.split(r"[,|]", q["keywords"]) if k.isdigit()}
            rows = [t for t in rows if want <= set(self._keywords(t))]
        rows = [t for t in rows if t["vote_count"] >= int(q.get("voteCountGte", 0))]
        sort = q.get("sortBy", "popularity.desc")
        if sort.startswith("vote_average"):
            rows.sort(key=lambda t: (-t["vote_average"], t["order"]))
        elif "release_date" in sort or "first_air_date" in sort:
            rows.sort(key=lambda t: (-t["year"], t["order"]))
        else:
            rows.sort(key=lambda t: t["order"])
        return rows

    # ---- details -------------------------------------------------------------------------------------
    def _detail(self, t):
        body = self._row(t)
        body.pop("genreIds"), body.pop("popularity")
        if t["kind"] == "movie" and t["title"] == "Tears of Steel":
            body["overview"] += TEARS_OF_STEEL_PLOT
        body.update(genres=self._genre_objs(t), tagline=t.get("tagline", ""), status="Released",
                    externalIds={"imdbId": None, "tvdbId": t["tvdb"]},
                    credits={"cast": [{"id": 5000 + n, "name": c["name"], "character": c.get("role", ""),
                                       "profilePath": None, "order": n}
                                      for n, c in enumerate(t.get("cast", []))],
                             "crew": [{"name": d, "job": "Director"} for d in t.get("directors", [])]},
                    relatedVideos=[], productionCompanies=[{"id": 1, "name": t.get("studio", "")}])
        if t["kind"] == "movie":
            body.update(runtime=t.get("minutes", 0),
                        releases={"results": [{"iso_3166_1": "US", "release_dates": [{"certification": "PG"}]}]})
        else:
            body.update(episodeRunTime=[3], numberOfSeasons=len(t["seasons"]),
                        numberOfEpisodes=sum(len(s["episodes"]) for s in t["seasons"]),
                        networks=[{"id": 1, "name": t.get("studio", "")}],
                        createdBy=[{"id": 6000, "name": c} for c in t.get("creators", [])],
                        contentRatings={"results": [{"iso_3166_1": "US", "rating": "TV-G"}]},
                        seasons=[{"seasonNumber": s["index"], "episodeCount": len(s["episodes"]),
                                  "name": f"Season {s['index']}", "posterPath": f"/demo/{t['slug']}/poster.jpg",
                                  "airDate": s["episodes"][0]["date"]} for s in t["seasons"]])
        info = self._media_info(t)
        if info:
            body["mediaInfo"] = info
        return body

    def _season(self, t, n):
        season = next((s for s in t["seasons"] if s["index"] == n), None)
        if season is None:
            return None
        return {"id": 7000 + n, "seasonNumber": n, "name": f"Season {n}",
                "episodes": [{"id": 7100 + e["index"], "episodeNumber": e["index"], "name": e["title"],
                              "airDate": e["date"], "overview": e["summary"],
                              "stillPath": f"/demo/{t['slug']}/still/{n}/{e['index']}.jpg"}
                             for e in season["episodes"]]}

    def _request_list(self, q):
        rows = []
        for n, (kind, title, rstatus, media) in list(enumerate(REQUESTS))[::-1]:
            t = self.by_title[(kind, title)]
            rows.append({"id": 190 + n, "status": rstatus, "type": kind, "is4k": False,
                         "seasons": [{"seasonNumber": 1, "status": rstatus}] if kind == "tv" else [],
                         "media": {"tmdbId": t["tmdb"], "tvdbId": t["tvdb"], "mediaType": kind, "status": media,
                                   "status4k": 1, "downloadStatus": self._download_status(t),
                                   "ratingKey": str(self._rating_key(t)) if media == 5 and self._rating_key(t) else None}})
        skip, take = int(q.get("skip", "0") or 0), int(q.get("take", "20") or 20)
        return {"pageInfo": {"pages": 1, "pageSize": take, "results": len(rows), "page": 1},
                "results": rows[skip:skip + take]}

    # ---- Radarr / Sonarr -----------------------------------------------------------------------------
    @staticmethod
    def _record(size, left, eta, **extra):
        rec = {"id": 1, "status": "downloading", "trackedDownloadState": "downloading",
               "trackedDownloadStatus": "ok", "size": size, "sizeleft": left, "timeleft": eta,
               "downloadId": "DEMODOWNLOAD", "downloadClient": "SABnzbd", "protocol": "usenet"}
        rec.update(extra)
        return rec

    def _radarr(self, p, q):
        movies = {t["tmdb"]: t for t in self.titles if t["kind"] == "movie" and t["title"] in MOVIE_QUEUE}
        if p == "/api/v3/system/status":
            return _json({"appName": "Radarr", "version": "5.14.0.9383"})
        if p == "/api/v3/movie":
            t = movies.get(int(q.get("tmdbId", "0") or 0))
            return _json([{"id": t["tmdb"] - 699_000, "tmdbId": t["tmdb"], "hasFile": False, "monitored": True,
                           "title": t["title"]}] if t else [])
        if p == "/api/v3/queue/details":
            mid = int(q.get("movieId", "0") or 0)
            return _json([self._record(*MOVIE_QUEUE[t["title"]], movieId=mid) for t in movies.values()
                          if t["tmdb"] - 699_000 == mid])
        if p == "/api/v3/queue":
            recs = [self._record(*MOVIE_QUEUE[t["title"]], movieId=t["tmdb"] - 699_000, movie={"tmdbId": t["tmdb"]})
                    for t in movies.values()]
            return _json({"page": 1, "pageSize": 100, "totalRecords": len(recs), "records": recs})
        return None

    def _sonarr(self, p, q):
        shows = {t["tvdb"]: t for t in self.titles if t["kind"] == "tv" and t["title"] in TV_QUEUE}
        sid = lambda t: t["tvdb"] - 899_000
        if p == "/api/v3/system/status":
            return _json({"appName": "Sonarr", "version": "4.0.10.2544"})
        if p == "/api/v3/series":
            t = shows.get(int(q.get("tvdbId", "0") or 0))
            if not t:
                return _json([])
            total = sum(len(s["episodes"]) for s in t["seasons"])
            return _json([{"id": sid(t), "tvdbId": t["tvdb"], "tmdbId": t["tmdb"], "title": t["title"],
                           "statistics": {"episodeFileCount": 0, "episodeCount": total},
                           "seasons": [{"seasonNumber": s["index"],
                                        "statistics": {"episodeFileCount": 0, "episodeCount": len(s["episodes"])}}
                                       for s in t["seasons"]]}])
        records = lambda t: [self._record(size, left, eta, seriesId=sid(t), id=n + 1,
                                          downloadId=f"DEMOSONARR{n + 1}",
                                          episode={"seasonNumber": s, "episodeNumber": e},
                                          series={"tmdbId": t["tmdb"], "tvdbId": t["tvdb"]},
                                          status="downloading" if left < size else "queued",
                                          trackedDownloadState="downloading")
                             for n, ((s, e), size, left, eta) in enumerate(TV_QUEUE[t["title"]])]
        if p == "/api/v3/queue/details":
            want = int(q.get("seriesId", "0") or 0)
            return _json([r for t in shows.values() if sid(t) == want for r in records(t)])
        if p == "/api/v3/queue":
            recs = [r for t in shows.values() for r in records(t)]
            return _json({"page": 1, "pageSize": 100, "totalRecords": len(recs), "records": recs})
        return None

    # ---- entry ---------------------------------------------------------------------------------------
    def handle(self, method, p, q, headers, body=b""):
        """`(status, content type, bytes, extra headers)` or None when `p` is not Seerr's or an Arr's."""
        if not p.startswith("/api/v1/") and not p.startswith("/api/v3/"):
            return None
        if p.startswith("/api/v3/"):
            key = headers.get("X-Api-Key", "")
            if key not in (RADARR_KEY, SONARR_KEY):
                return _json({"error": "Unauthorized"}, 401)
            answer = (self._radarr if key == RADARR_KEY else self._sonarr)(p, q)
            return answer if answer is not None else _json({"message": "NotFound"}, 404)
        rest = p[len("/api/v1/"):]
        if rest == "status":
            return _json({"version": "3.5.2", "commitTag": "$GIT_SHA", "updateAvailable": False, "commitsBehind": 0})
        if rest == "auth/plex" and method == "POST":
            return _json({"id": 1, "displayName": "Viewer", "permissions": 2, "userType": 1, "requestCount": len(REQUESTS)},
                         headers={"Set-Cookie": SESSION_COOKIE + "; Path=/; HttpOnly"})
        if rest == "settings/public":
            return _json({"initialized": True, "applicationTitle": "Seerr", "partialRequestsEnabled": True,
                          "enableSpecialEpisodes": False, "hideAvailable": True, "movie4kEnabled": False,
                          "series4kEnabled": False})
        if SESSION_COOKIE not in headers.get("Cookie", ""):
            return _json({"message": "Forbidden"}, 403)
        if rest == "auth/me":
            return _json({"id": 1, "displayName": "Viewer", "permissions": 2, "userType": 1,
                          "requestCount": len(REQUESTS)})
        if rest == "request" and method == "POST":
            req = json.loads(body or b"{}")
            self.posted.append(req)
            return _json({"id": 300 + len(self.posted), "status": 1, "type": req.get("mediaType")}, 201)
        if rest == "request":
            return _json(self._request_list(q))
        m = re.fullmatch(r"genres/(movie|tv)", rest)
        if m:
            used = sorted({g["id"] for t in self.titles if t["kind"] == m.group(1) for g in self._genre_objs(t)},
                          key=lambda i: GENRE_NAMES[i])
            return _json([{"id": i, "name": GENRE_NAMES[i]} for i in used])
        if rest == "discover/trending":
            movies = [t for t in self.titles if t["kind"] == "movie"]
            shows = [t for t in self.titles if t["kind"] == "tv"]
            mixed = []
            for n in range(len(movies)):
                mixed.append(movies[n])
                if n < len(shows):
                    mixed.append(shows[n])
            return _json(self._page(mixed, q))
        if rest == "discover/movies":
            return _json(dict(self._page(self._filtered("movie", q), q), keywords=[
                {"id": int(k), "name": KEYWORDS[int(k)][0]} for k in re.split(r"[,|]", q.get("keywords", ""))
                if k.isdigit() and int(k) in KEYWORDS]))
        if rest == "discover/tv":
            return _json(self._page(self._filtered("tv", q), q))
        if rest == "discover/movies/upcoming":
            rows = sorted([t for t in self.titles if t["kind"] == "movie" and t["year"] >= UPCOMING_FROM],
                          key=lambda t: t["year"])
            return _json(self._page(rows, q))
        if rest == "search/keyword":
            needle = q.get("query", "").casefold()
            rows = [{"id": k, "name": name} for k, (name, _) in KEYWORDS.items()
                    if needle and needle in name.casefold()]
            return _json({"page": 1, "totalPages": int(bool(rows)), "totalResults": len(rows), "results": rows})
        if rest == "search":
            needle = q.get("query", "").lower()
            return _json(self._page([t for t in self.titles if needle and needle in t["title"].lower()], q))
        m = re.fullmatch(r"(movie|tv)/(\d+)", rest)
        if m:
            t = self.by_key.get((m.group(1), int(m.group(2))))
            return _json(self._detail(t)) if t else _json({"message": "Not found"}, 404)
        m = re.fullmatch(r"tv/(\d+)/season/(\d+)", rest)
        if m:
            t = self.by_key.get(("tv", int(m.group(1))))
            season = self._season(t, int(m.group(2))) if t else None
            return _json(season) if season else _json({"message": "Not found"}, 404)
        return None
