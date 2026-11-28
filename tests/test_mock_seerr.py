"""The Seerr/Radarr/Sonarr stand-in: filters, session gate, download status.

Run: python3 -I tests/test_mock_seerr.py
"""
import json
import pathlib
import sys
import unittest
import urllib.parse

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import mock_seerr  # noqa: E402

CATALOG = json.loads((HERE / "demo_library" / "catalog.json").read_text())
COOKIE = {"Cookie": mock_seerr.SESSION_COOKIE}


def get(fx, path, headers=COOKIE):
    u = urllib.parse.urlsplit(path)
    q = {k: v[-1] for k, v in urllib.parse.parse_qs(u.query).items()}
    status, _, data, *_ = fx.handle("GET", u.path, q, headers)
    return status, json.loads(data)


class SeerrMock(unittest.TestCase):
    def setUp(self):
        self.fx = mock_seerr.SeerrFixture(CATALOG)

    def test_session_gate(self):
        self.assertEqual(get(self.fx, "/api/v1/discover/trending", {})[0], 403)
        self.assertEqual(get(self.fx, "/api/v1/status", {})[0], 200)

    def test_genre_decade_tag_filters_combine(self):
        _, page = get(self.fx, "/api/v1/discover/movies?genre=35&primaryReleaseDateGte=1920-01-01"
                               "&primaryReleaseDateLte=1929-12-31&keywords=10051")
        self.assertEqual(sorted(r["title"] for r in page["results"]),
                         ["Safety Last!", "Sherlock Jr.", "The General", "The Kid"])

    def test_download_is_matched_by_exact_ids(self):
        movie = next(t for t in self.fx.titles if t["title"] == "Tears of Steel")
        _, detail = get(self.fx, f"/api/v1/movie/{movie['tmdb']}")
        self.assertEqual(detail["mediaInfo"]["downloadStatus"][0]["status"], "downloading")
        radarr = {"X-Api-Key": mock_seerr.RADARR_KEY}
        _, found = get(self.fx, f"/api/v3/movie?tmdbId={movie['tmdb']}", radarr)
        _, queue = get(self.fx, f"/api/v3/queue/details?movieId={found[0]['id']}", radarr)
        self.assertEqual(queue[0]["size"] - queue[0]["sizeleft"], 2_976_000_000)
        self.assertEqual(get(self.fx, "/api/v3/queue", {})[0], 401)


if __name__ == "__main__":
    unittest.main()
