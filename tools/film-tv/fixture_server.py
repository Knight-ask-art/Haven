#!/usr/bin/env python3
"""Deterministic local source for film/TV runtime acceptance.

The server is deliberately a loopback-only test dependency.  It exposes a
small CMS10 catalog, an M3U playlist, a multi-level HLS presentation, a direct
MP4, subtitles, bounded failures, and redirect cases.  It never contacts the
Internet and does not write a runtime evidence record.

The production Tauri candidate must be built with the explicit
``film-tv-fixture`` feature before it can consume the loopback endpoint.  A
normal/release build continues to reject loopback destinations.
"""

from __future__ import annotations

import argparse
import base64
import gzip
import hashlib
import json
import re
import threading
import time
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import PurePosixPath
from urllib.error import HTTPError
from urllib.parse import parse_qs, quote, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener, urlopen


SCHEMA_VERSION = 1
FIXTURE_SET = "film-tv-fixture@1"
LOOPBACK_HOST = "127.0.0.1"
MAX_RESPONSE_BYTES = 8 * 1024 * 1024

# The media bytes are generated once from a one-second black H.264 sample and
# compressed only to keep this source reviewable.  Keeping the bytes in the
# fixture itself makes runtime playback independent of ffmpeg being installed
# on the developer or CI machine.
_DIRECT_MP4_GZIP_B64 = (
    "H4sICB7hmGoCA2RpcmVjdC5tcDQA1VVfaBxFGJ+7pInNFW1Lq6lEGLUVleayu0nOGLqQWqt5UAmoQYL1Mrc7l1uz/7Izlz9F5YQ+lOKTSNGLSB58sD4UfVFUxCA+tCA+SEFbpCjUolDUB/XR+JvNHdm7TaN98MG9/e1885tvZr755vu+I4TQslwKHRF4hGSJagGDzVu6Fw7phHS/4QXBPCHE9eYrNml5On6KkSHq3XgyrVrt/TGy5dOBd7eM2CzkZ+VsvGdHejW199b7bLrvCjAVd/dzWwq0fdwVcmNGY914bvaUZzsMAvXs9rMbMZ7+Iu70V2w3ao7MOzZPak6iH4wz33a50skOe45fhrBn3osXTZq5314f67MjXk4cY0c1cum6nPlDyJIL+SMhhZ3QOaMu7TquUAefIuNox5saex+DfsHIGyN5XdOp65QWjcJQYkbv2hq+A9A6knloNbf2A+mdQXup54Erqw+rO+lUn3OHJt6tZUhn5atbzkF9Z8hE2DBAYU9JRg3v9tHmwkLGnm8auk9FntLFgZK8Qh84q41rYhFjxxMGq3VeQ9vzH0MKaQUbe3ZfBg07el4B3gLeAz4HLgBXgd8JyXUBvcD9wCHgCWAaQETkasCrwNvABwD8mLsEXAP+wt3vgJ/f3yIjsptkRCZz45m4RUZ0NuZ23Hf9jPgE5/istllGiKDqJzWfRN9OZMSFRkbsFF7Lov8iG7KvN7LhpbZs8FG+WOrYGbKzaSqeAhd2HHAdtVrtAPzYifa2sb3x4ItAF98GZtuuk5M/ki5ImWxrSK/rKHckQlrpDMQuW3ef2itMhPFNbWGcbUDJ2xN8T4LfleB3J/h9Cf72BH+hLTU0QB2r83+MxUTa4azd5+EihE/PCeA08A7wMfAloFLyNwDpk8ON5+4CUNtyjwDPAM8DLwCngDeBs8Aq8DVwBfgTQYZ72nGr2kvMhLZyahS4btPHcW3sE6UZVeyaI5mGp1VbqtpSxd+Ux2UzDu9MJgUSKWJh6CYTo99Zz7wDZ2SgjnqHzeLJcQCjXpdRr3VD1WsVR+WII/Y6j3nQupwlLfWcZLWxm2HrU13b1tYmvjv686dXL45/ePrgt/Ti3b/8qio97adWEHGqF4ZpNGgYg1QbGtGskoaB8TwUBh6fOPpo/xA9PHkEmja3MHAkCJdcXpbU0LTBfkMzhkFWpAxHBwYWFhby6r8vcJmfD6KZAbVLviI9FzpBKJ3AF6PUYiVmmRpFRps6tXnJDaxZUxvFjzKfuUuCqx71uIlKQ0W1BEmjoViCOr7FyDb1vAYVfKjnLHK7qNZSM4oR82e4qReoVYkCjxUxVacy4q7rCGiMLI7YloRgzXn42pzZxwOfm4Z+UNdpmQlZDMWsE2JSY4G5sBiUy4KrSbISYYIwB6kbBLOsgk6xyelUuI7FNwiN+lG8heV4TCozHF/yyGVQAl9yqxFbKlqBF7LYIPgGFd7xsQQUI6Z0yhHzuFpqgTszFRlCmuVLGDaN4aZYROlElwqL+9yqqrXi+conERcV5WnLtKIy9VC2uHIlOqYxmNfonNre1PIFiKFaJ27Zoll4EIKQPDSHqBPCq7g7OH0INzRnakQVtBF+8sToPfeS7dnzzeebY9V/fp/L9NJuBVU5D9dp/uVrrcyY0c5Mp5haillJMauFdub7BhP3lkn7+DJNMWMpZjrF1FLMSopJ2bLcYks9ZUs9ZUs9ZUs9ZUs9ZUs9ZcsN+eVvKl22jB4MAAA="
)
_SEGMENT_TS_GZIP_B64 = (
    "H4sICB7hmGoCA3NlZ21lbnQudHMA1VldaBxVFL6TpInpNqabpm2KFW8p/iDJZmZ2M92mGd1tbDcPERZFfVG3d2fvZofMX2Ym2URq3IIPIhUEQWj2wSo+qPhQfelTYYsUoogvim2lSAPFHyg2vqes586kdkKL+ubcSbh7//fLme98OffcQi65Ex1dfxQJFxFqC20kbDQGpwaE7uPHTYfO9D5P3QVdo6JUPzn5TTteTyGHdiJ0ri/AjoR19OQXXV+2+XgKRcDece7hAPsaWkcPsaJ/TVjv3t41b1XQA6//Vo8r9jwSe4oILX+7zOx+HaFGY9sB1PMrYc3edVZ2b2u3i9eO/X7hl6tT598fvoKvHvzj1qKsZPAI1myXYkkZw25altNYzGRFrSzCwFQKJow+WzxWGMng/IuTMLNCNRiYtJ0lg1Z9LItiekQW5THorPm+Mz46Wq/XUwt6hdoGsVK2OzPKviVV800D5tiOr9uWN441UiaaKuKCgJIuraoSrtCyYWuzqjgOP5hYxFjyKGthk6oVnWBvvgw1ETveEkyHsuRWVCklwhQosKkv0kqJ7cVWlFxizVBVUrBWc22TlGCphH2XGobuwYzsYrai+VDR5kwoK5RUXrMtqsrSsCThKvH8kuPN6g4s2txgzinZ1apH2SK/5sICwD7gqWls2PYsqUFHKez3YJFngJfe7RCx5QZfo+km8RkU3fKpaxCYBP1lY94lSyXNNh0SgAIb+S7RLdgCJrqEzam6xKRsqzrVZ2q+A7VZugTDqjx2p1oydQua2NOoRbV5tlewntnFpV4N2q6mam4VsKd3oAiJsFkG6zD7wqgqp1MinmN4VDGlQNVhGwefZFFVDkPF86mjZrDugKnhpcKbyMBrm1NF1GCEmznaSvy0/dCN1jMIdaIuVqxOFD8Lxmrf9a/CB33rzfHHnkC9HV/feS6/Mv/vv68C3zNnUFyVJOKBvZ2n7npgfgWnTt0E7GNnQCVPX1tG8cb+yIdR7DkZsOdQkmN9T3Ks7woffD9yMcqZE3LA90OM7ys34873l9ai2Bsh9iwfdveFKPazoa8OcOyrAxz76mHG90824s73dw9EOdNSAr6P88H3z/NR7NdD7EeY3c/viLvdVyNReb6JAuyCKOSg2UI/hJH7FdL+czrX+cHGzx1omixoipySsylJlFCHmHuQjQlvbAhDuOd/rxYEQXovhhb/L38DaOQujjVyF8caOcGHztyIxu9NHOqMynTm0v6Y60wCReP3Zi7E/hQXdk/sj8aRzRNKEM8Mcuyrgxz76tOM798Px53v2Wj83mwEnEkmuc2l5pHIh6++ED17NM+GOiMxzqxNxJ0zc9EYuNkKdWY3xzqzm2Odkfng+zvRs0dz8+yRZny/NR13vn8aPXushGcPTnKpiUtbc6nhuUnePDd9FcZrl0mczkb/VIVzU/pjxIGo3P/ctIdjjdzDsUYGdwe3X467zqxtvTsIdYaPHHbi9tYctvJ3DltIGDG3e9++rTnsMJ7Zy7Gv7uXYV/m4O+hTtt4dhHxnOWxh38m48/05fnPYfda9OewcGuLYV4c49lV2dyA8fjrufH/7fncHmf7cPSta6KPwv9mPMY6I/wJwnluM7CYAAA=="
)


def _decode_asset(encoded: str) -> bytes:
    data = gzip.decompress(base64.b64decode(encoded, validate=True))
    if not data or len(data) > MAX_RESPONSE_BYTES:
        raise RuntimeError("fixture media asset is empty or exceeds the response budget")
    return data


DIRECT_MP4 = _decode_asset(_DIRECT_MP4_GZIP_B64)
SEGMENT_TS = _decode_asset(_SEGMENT_TS_GZIP_B64)


def _base_url(server: ThreadingHTTPServer) -> str:
    return f"http://{LOOPBACK_HOST}:{server.server_address[1]}"


def _json_bytes(value: object) -> bytes:
    return (json.dumps(value, ensure_ascii=False, separators=(",", ":")) + "\n").encode("utf-8")


def _asset_sha256(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


class FixtureHandler(BaseHTTPRequestHandler):
    """HTTP surface for one fixed fixture set."""

    protocol_version = "HTTP/1.1"
    server_version = "HavenFilmTvFixture/1"
    sys_version = ""

    def do_GET(self) -> None:  # noqa: N802 - stdlib handler API
        self._handle(write_body=True)

    def do_HEAD(self) -> None:  # noqa: N802 - stdlib handler API
        self._handle(write_body=False)

    def log_message(self, format: str, *args: object) -> None:
        if getattr(self.server, "verbose", False):
            route = urlsplit(self.path).path
            print(f"[fixture] {self.command} {route}", flush=True)

    @property
    def fixture_server(self) -> ThreadingHTTPServer:
        return self.server  # type: ignore[return-value]

    def _handle(self, *, write_body: bool) -> None:
        parsed = urlsplit(self.path)
        route = str(PurePosixPath(parsed.path or "/"))
        query = parse_qs(parsed.query, keep_blank_values=True)

        if route == "/healthz":
            self._send_bytes(b"ok\n", "text/plain; charset=utf-8", write_body=write_body)
            return
        if route == "/fixture-manifest.json":
            self._send_bytes(_json_bytes(self._manifest()), "application/json", write_body=write_body)
            return
        if route == "/cms10/api.php":
            self._handle_cms10(query, write_body=write_body)
            return
        if route == "/m3u/playlist.m3u":
            self._send_text(self._m3u(), "audio/x-mpegurl", write_body=write_body)
            return
        if route == "/hls/master.m3u8":
            self._send_text(self._hls_master(), "application/vnd.apple.mpegurl", write_body=write_body)
            return
        if route == "/hls/variants/low/index.m3u8":
            self._send_text(self._hls_variant(), "application/vnd.apple.mpegurl", write_body=write_body)
            return
        if route == "/hls/subtitles/zh.vtt":
            self._send_text(self._subtitle(), "text/vtt; charset=utf-8", write_body=write_body)
            return
        if route == "/hls/segments/segment-0.ts":
            self._send_bytes(SEGMENT_TS, "video/mp2t", write_body=write_body)
            return
        if route == "/media/direct.mp4":
            self._send_bytes(DIRECT_MP4, "video/mp4", write_body=write_body)
            return
        if route == "/failure/manifest.m3u8":
            self._send_error(HTTPStatus.SERVICE_UNAVAILABLE, "fixture-manifest-failure", write_body=write_body)
            return
        if route == "/failure/segment.ts":
            self._send_error(HTTPStatus.BAD_GATEWAY, "fixture-segment-failure", write_body=write_body)
            return
        if route in {"/failure/slow", "/failure/slow.m3u8"}:
            time.sleep(0.25)
            self._send_error(HTTPStatus.GATEWAY_TIMEOUT, "fixture-timeout", write_body=write_body)
            return
        if route in {"/redirect/safe", "/redirect/safe.m3u8"}:
            self._send_redirect("/hls/master.m3u8")
            return
        if route in {"/redirect/blocked", "/redirect/blocked.m3u8"}:
            # Port 1 is intentionally outside the application allowlist.  The
            # Tauri proxy must reject this hop before opening another socket.
            self._send_redirect("http://127.0.0.1:1/blocked")
            return
        self._send_error(HTTPStatus.NOT_FOUND, "fixture-not-found", write_body=write_body)

    def _handle_cms10(self, query: dict[str, list[str]], *, write_body: bool) -> None:
        if query.get("ac", [""])[0] != "videolist":
            self._send_error(HTTPStatus.BAD_REQUEST, "fixture-cms10-query", write_body=write_body)
            return
        item_id = query.get("ids", [""])[0].strip()
        search = query.get("wd", [""])[0].strip().lower()
        items = [self._cms_item("fixture-series", "Fixture Series", "剧集")]
        items.append(self._cms_item("fixture-movie", "Fixture Movie", "电影"))
        if item_id:
            items = [item for item in items if item["vod_id"] == item_id]
        elif search and search not in {"fixture", "示例", "series", "movie"}:
            items = [item for item in items if search in item["vod_name"].lower()]
        self._send_bytes(
            _json_bytes({"code": 1, "list": items}),
            "application/json",
            write_body=write_body,
        )

    def _cms_item(self, item_id: str, title: str, type_name: str) -> dict[str, object]:
        base = _base_url(self.fixture_server)
        if type_name == "电影":
            play_url = f"正片${base}/media/direct.mp4"
        else:
            play_url = (
                f"第01集${base}/hls/master.m3u8#"
                f"第02集${base}/media/direct.mp4"
            )
        return {
            "vod_id": item_id,
            "vod_name": title,
            "type_name": type_name,
            "vod_year": "2026",
            "vod_play_url": play_url,
            "vod_content": "受控本地 fixture；不依赖第三方站点。",
            "vod_director": "Fixture Director",
            "vod_actor": "Fixture Actor",
        }

    def _m3u(self) -> str:
        base = _base_url(self.fixture_server)
        return (
            "#EXTM3U\n"
            '#EXTINF:-1 tvg-name="Fixture Series" group-title="Fixture",Fixture Series\n'
            f"{base}/hls/master.m3u8\n"
            '#EXTINF:-1 tvg-name="Fixture Direct" group-title="Fixture",Fixture Direct\n'
            f"{base}/media/direct.mp4\n"
            '#EXTINF:-1 tvg-name="Fixture Failure Manifest" group-title="Fixture",Fixture Failure Manifest\n'
            f"{base}/failure/manifest.m3u8\n"
            '#EXTINF:-1 tvg-name="Fixture Failure Segment" group-title="Fixture",Fixture Failure Segment\n'
            f"{base}/failure/segment.ts\n"
            '#EXTINF:-1 tvg-name="Fixture Safe Redirect" group-title="Fixture",Fixture Safe Redirect\n'
            f"{base}/redirect/safe.m3u8\n"
            '#EXTINF:-1 tvg-name="Fixture Blocked Redirect" group-title="Fixture",Fixture Blocked Redirect\n'
            f"{base}/redirect/blocked.m3u8\n"
            '#EXTINF:-1 tvg-name="Fixture Slow Failure" group-title="Fixture",Fixture Slow Failure\n'
            f"{base}/failure/slow.m3u8\n"
            "#EXTINF:-1,Malformed\n"
            "file:///not-allowed\n"
        )

    def _hls_master(self) -> str:
        return (
            "#EXTM3U\n"
            "#EXT-X-VERSION:3\n"
            '#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID="subs",NAME="简体中文",'
            'LANGUAGE="zh-CN",DEFAULT=YES,AUTOSELECT=YES,URI="subtitles/zh.vtt"\n'
            '#EXT-X-STREAM-INF:BANDWIDTH=256000,CODECS="avc1.42E01E",'
            'RESOLUTION=160x90,SUBTITLES="subs"\n'
            "variants/low/index.m3u8\n"
        )

    def _hls_variant(self) -> str:
        return (
            "#EXTM3U\n"
            "#EXT-X-VERSION:3\n"
            "#EXT-X-TARGETDURATION:1\n"
            "#EXT-X-MEDIA-SEQUENCE:0\n"
            "#EXTINF:1.0,\n"
            "../../segments/segment-0.ts\n"
            "#EXT-X-ENDLIST\n"
        )

    def _subtitle(self) -> str:
        return (
            "WEBVTT\n"
            "\n"
            "00:00.000 --> 00:00.900\n"
            "受控 fixture 字幕\n"
        )

    def _manifest(self) -> dict[str, object]:
        return {
            "schemaVersion": SCHEMA_VERSION,
            "fixtureSet": FIXTURE_SET,
            "allowedHost": LOOPBACK_HOST,
            "paths": {
                "cms10": "/cms10/api.php",
                "m3u": "/m3u/playlist.m3u",
                "hlsMaster": "/hls/master.m3u8",
                "hlsVariant": "/hls/variants/low/index.m3u8",
                "subtitle": "/hls/subtitles/zh.vtt",
                "directVideo": "/media/direct.mp4",
                "failureManifest": "/failure/manifest.m3u8",
                "failureSegment": "/failure/segment.ts",
                "slow": "/failure/slow",
                "safeRedirect": "/redirect/safe",
                "blockedRedirect": "/redirect/blocked",
            },
            "assets": {
                "directMp4Sha256": _asset_sha256(DIRECT_MP4),
                "segmentTsSha256": _asset_sha256(SEGMENT_TS),
            },
        }

    def _send_text(self, value: str, content_type: str, *, write_body: bool) -> None:
        self._send_bytes(value.encode("utf-8"), content_type, write_body=write_body)

    def _send_error(self, status: HTTPStatus, message: str, *, write_body: bool) -> None:
        self._send_bytes(
            (message + "\n").encode("utf-8"),
            "text/plain; charset=utf-8",
            status=status,
            write_body=write_body,
        )

    def _send_redirect(self, location: str) -> None:
        self.send_response(HTTPStatus.FOUND)
        self.send_header("Location", location)
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Content-Length", "0")
        self.end_headers()

    def _send_bytes(
        self,
        value: bytes,
        content_type: str,
        *,
        status: HTTPStatus = HTTPStatus.OK,
        write_body: bool,
    ) -> None:
        if len(value) > MAX_RESPONSE_BYTES:
            self._send_error(HTTPStatus.INTERNAL_SERVER_ERROR, "fixture-size-error", write_body=write_body)
            return

        body = value
        content_range: str | None = None
        range_header = self.headers.get("Range")
        if range_header:
            parsed = re.fullmatch(r"bytes=(\d*)-(\d*)", range_header.strip())
            if not parsed or (not parsed.group(1) and not parsed.group(2)):
                self._send_range_error(len(value), write_body=write_body)
                return
            if parsed.group(1):
                start = int(parsed.group(1))
                end = int(parsed.group(2)) if parsed.group(2) else len(value) - 1
            else:
                suffix_length = int(parsed.group(2))
                start = max(0, len(value) - suffix_length)
                end = len(value) - 1
            if start >= len(value) or start > end or end < 0:
                self._send_range_error(len(value), write_body=write_body)
                return
            end = min(end, len(value) - 1)
            body = value[start : end + 1]
            status = HTTPStatus.PARTIAL_CONTENT
            content_range = f"bytes {start}-{end}/{len(value)}"

        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Accept-Ranges", "bytes")
        if content_range:
            self.send_header("Content-Range", content_range)
        self.end_headers()
        if write_body:
            self.wfile.write(body)

    def _send_range_error(self, size: int, *, write_body: bool) -> None:
        self.send_response(HTTPStatus.REQUESTED_RANGE_NOT_SATISFIABLE)
        self.send_header("Content-Range", f"bytes */{size}")
        self.send_header("Content-Length", "0")
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.end_headers()


class _RejectRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # type: ignore[no-untyped-def]
        return None


def _make_server(port: int, *, verbose: bool = False) -> ThreadingHTTPServer:
    server = ThreadingHTTPServer((LOOPBACK_HOST, port), FixtureHandler)
    server.daemon_threads = True
    server.verbose = verbose  # type: ignore[attr-defined]
    return server


def _read(url: str, *, headers: dict[str, str] | None = None) -> tuple[int, dict[str, str], bytes]:
    request = Request(url, headers=headers or {})
    with urlopen(request, timeout=3) as response:
        return response.status, dict(response.headers.items()), response.read(MAX_RESPONSE_BYTES + 1)


def _expect_status(url: str, status: int) -> tuple[dict[str, str], bytes]:
    try:
        _read(url)
    except HTTPError as error:
        body = error.read(MAX_RESPONSE_BYTES + 1)
        if error.code != status:
            raise AssertionError(f"expected {status} from {url}, got {error.code}") from error
        return dict(error.headers.items()), body
    raise AssertionError(f"expected HTTP {status} from {url}")


def run_self_check() -> None:
    """Exercise the server contract without creating runtime evidence."""

    server = _make_server(0)
    thread = threading.Thread(target=server.serve_forever, name="film-tv-fixture-check", daemon=True)
    thread.start()
    base = _base_url(server)
    try:
        status, _, body = _read(base + "/healthz")
        assert status == 200 and body == b"ok\n"

        status, _, body = _read(base + "/fixture-manifest.json")
        manifest = json.loads(body)
        assert status == 200
        assert manifest["schemaVersion"] == SCHEMA_VERSION
        assert manifest["fixtureSet"] == FIXTURE_SET
        assert manifest["allowedHost"] == LOOPBACK_HOST

        status, _, body = _read(base + "/cms10/api.php?ac=videolist&wd=" + quote("fixture"))
        cms = json.loads(body)
        assert status == 200 and len(cms["list"]) == 2
        assert "/hls/master.m3u8" in cms["list"][0]["vod_play_url"]

        status, _, body = _read(base + "/cms10/api.php?ac=videolist&ids=fixture-movie")
        assert status == 200 and json.loads(body)["list"][0]["vod_id"] == "fixture-movie"

        status, _, body = _read(base + "/m3u/playlist.m3u")
        playlist = body.decode("utf-8")
        assert status == 200 and playlist.startswith("#EXTM3U")
        assert "file:///not-allowed" in playlist
        for path in [
            "/failure/manifest.m3u8",
            "/failure/segment.ts",
            "/redirect/safe.m3u8",
            "/redirect/blocked.m3u8",
            "/failure/slow.m3u8",
        ]:
            assert path in playlist

        status, _, body = _read(base + "/hls/master.m3u8")
        master = body.decode("utf-8")
        assert status == 200 and 'URI="subtitles/zh.vtt"' in master
        assert "variants/low/index.m3u8" in master

        status, _, body = _read(base + "/hls/variants/low/index.m3u8")
        assert status == 200 and "../../segments/segment-0.ts" in body.decode("utf-8")

        status, headers, body = _read(
            base + "/media/direct.mp4",
            headers={"Range": "bytes=0-15"},
        )
        assert status == 206 and len(body) == 16
        assert headers.get("Content-Range") == f"bytes 0-15/{len(DIRECT_MP4)}"
        assert DIRECT_MP4[:8] == b"\x00\x00\x00\x20ftyp"
        assert SEGMENT_TS[0] == 0x47

        _expect_status(base + "/failure/manifest.m3u8", 503)
        _expect_status(base + "/failure/segment.ts", 502)
        _expect_status(base + "/failure/slow", 504)

        status, _, _ = _read(base + "/redirect/safe")
        assert status == 200
        opener = build_opener(_RejectRedirect())
        try:
            opener.open(base + "/redirect/blocked", timeout=3)
        except HTTPError as error:
            assert error.code == 302
            assert error.headers.get("Location") == "http://127.0.0.1:1/blocked"
        else:
            raise AssertionError("blocked redirect was followed by the fixture checker")
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=3)


def _serve(server: ThreadingHTTPServer) -> None:
    base = _base_url(server)
    output = {
        "schemaVersion": SCHEMA_VERSION,
        "fixtureSet": FIXTURE_SET,
        "host": LOOPBACK_HOST,
        "port": server.server_address[1],
        "cms10Endpoint": base + "/cms10/api.php",
        "m3uEndpoint": base + "/m3u/playlist.m3u",
        "manifest": base + "/fixture-manifest.json",
    }
    print(json.dumps(output, ensure_ascii=False, separators=(",", ":")), flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.shutdown()
        server.server_close()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Haven film/TV loopback fixture source")
    parser.add_argument("--check", action="store_true", help="run the fixture self-check and exit")
    parser.add_argument("--serve", action="store_true", help="serve until interrupted (the default)")
    parser.add_argument("--port", type=int, default=0, help="loopback port; 0 chooses an ephemeral port")
    parser.add_argument("--verbose", action="store_true", help="print route-only request logs")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.port < 0 or args.port > 65535:
        raise SystemExit("--port must be between 0 and 65535")
    if args.check:
        run_self_check()
        print(f"film-tv fixture self-check PASS ({FIXTURE_SET})")
        return 0
    _serve(_make_server(args.port, verbose=args.verbose))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
