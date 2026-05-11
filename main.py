# Copyright 2022 Thijs Raymakers
#
# Licensed under the EUPL, Version 1.2 or – as soon they
# will be approved by the European Commission - subsequent
# versions of the EUPL (the "Licence");
# You may not use this work except in compliance with the
# Licence.
# You may obtain a copy of the Licence at:
#
# https://joinup.ec.europa.eu/software/page/eupl
#
# Unless required by applicable law or agreed to in
# writing, software distributed under the Licence is
# distributed on an "AS IS" basis,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either
# express or implied.
# See the Licence for the specific language governing
# permissions and limitations under the Licence.

import asyncio
import re
import sys
import logging
from os import getenv
from podimo.client import PodimoClient
from feedgen.feed import FeedGenerator
from mimetypes import guess_type
from aiohttp import ClientSession, ClientError, CookieJar, ClientTimeout
from quart import Quart, Response, render_template, request
from hypercorn.config import Config
from hypercorn.asyncio import serve
from urllib.parse import quote
from podimo.config import (
    BLOCKED,
    CACHE_DIR,
    DEBUG,
    HEAD_CACHE_TIME,
    HTTP_PROXY,
    LOCAL_CREDENTIALS,
    LOCALES,
    PODCAST_CACHE_TIME,
    PODIMO_BIND_HOST,
    PODIMO_EMAIL,
    PODIMO_HOSTNAME,
    PODIMO_PASSWORD,
    PODIMO_PROTOCOL,
    PUBLIC_FEEDS,
    REGIONS,
    SCRAPER_API,
    STORE_TOKENS_ON_DISK,
    TOKEN_CACHE_TIME,
    ZENROWS_API,
)
from podimo.utils import generateHeaders, randomHexId
import podimo.cache as cache
import cloudscraper
import traceback

# Setup Quart, used for serving the web pages
app = Quart(__name__)
proxies = dict()
scraper = cloudscraper.create_scraper()

def example():
    return f"""Example
------------
Username: example@example.com
Password: this-is-my-password
Podcast ID: 12345-abcdef

The URL will be
https://example%40example.com:this-is-my-password@{PODIMO_HOSTNAME}/feed/12345-abcdef.xml

Note that the username and password should be URL encoded. This can be done with
a tool like https://gchq.github.io/CyberChef/#recipe=URL_Encode(true)
"""

@app.after_request
def allow_cors(response):
    # Scope CORS to safe, read-only feed fetches. The form endpoint `/` must
    # stay same-origin (POSTing credentials cross-origin would be unsafe), and
    # POST is never a legitimate cross-origin operation on this app.
    if request.method in ('GET', 'HEAD') and request.path.startswith('/feed/'):
        response.headers.set('Access-Control-Allow-Origin', '*')
        response.headers.set('Access-Control-Allow-Methods', 'GET, HEAD')

    # Only cache successful responses. Caching 401/404/410/500 on every CDN/
    # proxy for 15 minutes turns a transient upstream blip into a sticky outage.
    # /healthz must never be cached so liveness probes always reflect current state.
    if request.path == "/healthz":
        response.headers.set('Cache-Control', 'no-store')
    elif 200 <= response.status_code < 300:
        response.headers.set('Cache-Control', 'max-age=900')
    else:
        response.headers.set('Cache-Control', 'no-store')

    logging.debug(f"Incoming {request.method} {request.path} from User-Agent {request.user_agent} at {request.remote_addr}.")
    return response

def authenticate():
    return Response(
        f"""401 Unauthorized.
You need to login with the correct credentials for Podimo.

{example()}""",
        401,
        {
            "Content-Type": "text/plain",
            "WWW-Authenticate": "Basic realm='Podimo credentials'"
        },
    )

def initialize_client(username: str, password: str, region: str, locale: str) -> PodimoClient:
    client = PodimoClient(username, password, region, locale)

    # Check if there is an authentication token already in memory. If so, use that one.
    # If it is expired, request a new token.
    key = client.key
    client.token = cache.getCacheEntry(key, cache.TOKENS)

    # Check if we previously created a cookie jar
    if key not in cache.cookie_jars:
        cache.cookie_jars[key] = CookieJar()
    client.cookie_jar = cache.cookie_jars[key]
    return client

async def check_auth(username, password, region, locale, scraper):
    # Only ValueError represents genuinely bad credentials (malformed email in
    # PodimoClient.__init__, or rejection from podimoLogin). Network failures,
    # Cloudflare blocks, and upstream GraphQL errors propagate so the caller
    # can distinguish "wrong password" (401) from "upstream unavailable" (503).
    try:
        client = initialize_client(username, password, region, locale)
        if client.token:
            return client

        await client.podimoLogin(scraper)
        cache.insertIntoTokenCache(client.key, client.token)
        return client

    except ValueError as e:
        logging.info(f"Auth rejected: {e}")
        if DEBUG:
            traceback.print_exc()
        return None

podcast_id_pattern = re.compile(r"[0-9a-fA-F\-]+")

@app.route("/", methods=["POST", "GET"])
async def index():
    error = ""
    if request.method == "POST":
        form = await request.form
        email = form.get("email")
        password = form.get("password")
        podcast_id = form.get("podcast_id")
        region = form.get("region")
        locale = form.get("locale")

        if not LOCAL_CREDENTIALS:
            if email is None or email == "":
                error += "Email is required"
            if password is None or password == "":
                error += "Password is required"
        if podcast_id is None or podcast_id == "":
            error += "Podcast ID is required"
        elif podcast_id_pattern.fullmatch(podcast_id) is None:
            error += "Podcast ID is not valid"
        if region is None or region == "":
            error += "Region is required"
        elif region not in [region_code for (region_code, _) in REGIONS]:
            error += "Region is not valid"
        if locale is None or locale == "":
            error += "Locale is required"
        elif locale not in LOCALES:
            error += "Locale is not valid"

        if error == "":
            podcast_id = quote(str(podcast_id), safe="")
            region = quote(str(region), safe="")
            locale = quote(str(locale), safe="")
            
            if LOCAL_CREDENTIALS:
                url = f"{PODIMO_PROTOCOL}://{PODIMO_HOSTNAME}/feed/{podcast_id}.xml?{randomHexId(10)}&region={region}&locale={locale}"
            else:
                email = quote(str(email), safe="")
                comma = quote(',', safe="")
                username = f"{email}{comma}{region}{comma}{locale}"
                password = quote(str(password), safe="")             
                url = f"{PODIMO_PROTOCOL}://{username}:{password}@{PODIMO_HOSTNAME}/feed/{podcast_id}.xml?{randomHexId(10)}&region={region}&locale={locale}"
            
            logging.debug(f"Created feed URL for podcast {podcast_id} (region={region}, locale={locale}, local_credentials={LOCAL_CREDENTIALS}).")
            return await render_template("feed_location.html", url=url)

    return await render_template("index.html", error=error, locales=LOCALES, regions=REGIONS, need_credentials=not(LOCAL_CREDENTIALS))


@app.route("/healthz", methods=["GET"])
async def healthz():
    return Response('{"status":"ok"}', 200, {"Content-Type": "application/json"})


@app.errorhandler(404)
async def not_found(error):
    return Response(
        f"404 Not found.\n\n{example()}", 404, {"Content-Type": "text/plain"}
    )


def _arg(args, name):
    # Some downstream tools (e.g. Audiobookshelf) consume the feed URL as
    # rendered HTML and don't decode entities, so `&amp;region=...` arrives as a
    # parameter literally named `amp;region`. Accept either form.
    return args.get(name) or args.get(f"amp;{name}")


@app.route("/feed/<string:podcast_id>.xml")
async def serve_basic_auth_feed(podcast_id):
    if LOCAL_CREDENTIALS:
        args = request.args
        region = _arg(args, "region") or "nl"
        locale = _arg(args, "locale") or "nl-NL"
        return await serve_feed(PODIMO_EMAIL, PODIMO_PASSWORD, podcast_id, region, locale)
    else:
        auth = request.authorization
        if not auth:
            return authenticate()
        else:
            username, region, locale = split_username_region_locale(auth.username)
            return await serve_feed(username, auth.password, podcast_id, region, locale)


def split_username_region_locale(string):
    s = string.split(',')
    if len(s) == 3:
        return tuple(s)
    else:
        return (s[0], 'nl', 'nl-NL')


async def serve_feed(username, password, podcast_id, region, locale):
    
    logging.debug(f"Feed request for podcast {podcast_id} from IP {request.remote_addr} with User-Agent:{request.user_agent}.")
    
    # Check if it is a valid podcast id string
    if podcast_id_pattern.fullmatch(podcast_id) is None:
        return Response("Invalid podcast id format", 400, {})
   
    if region not in [region_code for (region_code, _) in REGIONS]:
        return Response("Invalid region", 400, {})
    if locale not in LOCALES:
        return Response("Invalid locale", 400, {})

    # Check if url contains unique ID or podcastID in blocked list. If so, return HTTP code 410 GONE
    if any(item in request.url for item in BLOCKED):
        logging.debug(f"Blocked! Podcast {podcast_id} is on local block list")
        return Response("Podcast is gone", 410, {}) 
    
    try:
        client = await check_auth(username, password, region, locale, scraper)
    except ValueError as e:
        # Defensive: check_auth already catches ValueError, but if it ever
        # leaks one (or is replaced/wrapped), bad creds still map to 401.
        logging.info(f"Auth rejected at call site: {e}")
        return authenticate()
    except Exception as e:
        # check_auth only swallows ValueError (bad creds). Anything else here
        # is a transient upstream/network failure — return 503 so clients
        # retry instead of showing the user a misleading 401.
        logging.error(f"Upstream auth failure: {e}")
        if DEBUG:
            traceback.print_exc()
        return Response("Upstream temporarily unavailable, please retry", 503, {})
    if not client:
        return authenticate()

    # Get a list of valid podcasts
    try:
        podcasts = await podcastsToRss(
            podcast_id, await client.getPodcasts(podcast_id, scraper), locale
        )
    except Exception as e:
        exception = str(e)
        if "not found" in exception.lower():
            return Response(
                "Podcast not found. Are you sure you have the correct ID?", 404, {}
            )
        logging.error(f"Error while fetching podcasts: {exception}")
        return Response("Something went wrong while fetching the podcasts", 500, {})
    return Response(podcasts, mimetype="text/xml")


async def urlHeadInfo(session, id, url, locale):
    entry = cache.getHeadEntry(id)
    if entry:
        return entry

    retries = 3  # Number of retries
    timeout = ClientTimeout(total=10)  # 10 seconds timeout for each try

    for attempt in range(retries):
        try:
            logging.debug(f"HEAD request to {url} (Attempt {attempt + 1})")
            async with session.head(url, allow_redirects=True,
                                    headers=generateHeaders(None, locale),
                                    timeout=timeout) as response:
                content_length = 0
                content_type, _ = guess_type(url)
                if 'content-length' in response.headers:
                    content_length = response.headers['content-length']
                if content_type is None:
                    if 'content-type' in response.headers:
                        content_type = response.headers['content-type']
                    else:
                        content_type = 'audio/mpeg'
                cache.insertIntoHeadCache(id, content_length, content_type)
                return (content_length, content_type)

        except (asyncio.TimeoutError, ClientError) as exc:
            if attempt < retries - 1:
                logging.info(f"Retrying HEAD {url} after {type(exc).__name__} (attempt {attempt + 2}/{retries})")
                await asyncio.sleep(2 ** attempt)
            else:
                logging.error(f"All retries failed for HEAD request to {url} ({type(exc).__name__})")
                raise  # Re-raise the last exception if all retries fail



def extract_audio_url(episode):
    duration = 0
    url = None
    if episode['audio']:
        url = episode['audio']['url']
        duration = episode['audio']['duration']

    if url is None or url == "":
        if episode["streamMedia"]:
            url = episode["streamMedia"]["url"]
            duration = episode["streamMedia"]["duration"]
            if "hls-media" in url and "/main.m3u8" in url:
                url = url.replace("hls-media", "audios")
                url = url.replace("/main.m3u8", ".mp3")

    return url, duration


async def addFeedEntry(fg, episode, session, locale):
    fe = fg.add_entry()
    fe.guid(episode["id"])
    fe.title(episode["title"])
    fe.description(episode["description"])
    fe.pubDate(episode.get("publishDatetime", episode.get("datetime")))
    image_url = episode.get("imageUrl")
    if image_url:
        # Podimo image URLs end in a signed query string, not a file extension.
        # feedgen (and Apple's spec) require the URL to end in .jpg/.png — append
        # a fragment that satisfies the check without affecting image fetches,
        # since clients strip the fragment before the HTTP GET.
        if not image_url.lower().endswith(('.jpg', '.png')):
            image_url = image_url + '#.jpg'
        fe.podcast.itunes_image(image_url)

    url, duration = extract_audio_url(episode)
    if url is None:
        return 
    logging.debug(f"Found podcast '{episode['title']}'")
    fe.podcast.itunes_duration(duration)
    content_length, content_type = await urlHeadInfo(session, episode['id'], url, locale)
    fe.enclosure(url, content_length, content_type)

def chunks(x, n):
    for i in range(0, len(x), n):
        yield x[i:i + n]

async def podcastsToRss(podcast_id, data, locale):
    fg = FeedGenerator()
    fg.load_extension("podcast")

    podcast = data["podcast"]
    episodes = data["episodes"]

    if len(episodes) > 0:
        last_episode = episodes[0]
        title = podcast["title"]
        if podcast["title"] is None:
            title = last_episode["podcastName"]
        fg.title(title)

        if podcast["description"]:
            fg.description(podcast["description"])
        else:
            fg.description(title)

        fg.link(href=f"https://podimo.com/shows/{podcast_id}", rel="alternate")

        image = podcast["images"]["coverImageUrl"]
        if image is None:
            image = last_episode['imageUrl']
        if image and not image.lower().endswith(('.jpg', '.png')):
            image = image + '#.jpg'
        fg.image(image)

        language = podcast["language"]
        if language is None:
            language = locale
        fg.language(language)

        artist = podcast["authorName"]
        if artist is None:
            artist = last_episode["artist"]
        fg.podcast.itunes_author(artist)

        if not PUBLIC_FEEDS:
            fg.podcast.itunes_block(True)

    async with ClientSession() as session:
        for chunk in chunks(episodes, 5):
            results = await asyncio.gather(
                *[addFeedEntry(fg, episode, session, locale) for episode in chunk],
                return_exceptions=True
            )
            for episode, result in zip(chunk, results):
                if isinstance(result, Exception):
                    logging.warning(f"Failed to add feed entry for episode {episode['id']}: {result}")

    feed = fg.rss_str(pretty=True)
    return feed


async def spawn_web_server():
    config = Config()
    config.bind = [PODIMO_BIND_HOST]
    config.read_timeout = 60
    config.graceful_timeout = 5
    config.backlog = 1000
    app.config['TEMPLATES_AUTO_RELOAD'] = True
    await serve(app, config)

async def main():
    if HTTP_PROXY:
        global proxies
        logging.info(f"Running with https proxy defined in environmental variable HTTP_PROXY: {HTTP_PROXY}")
        proxies['https'] = HTTP_PROXY
        scraper.proxies = proxies
    await spawn_web_server()

if __name__ == "__main__":
    if DEBUG:
        logging.info(f"""Spawning server on {PODIMO_BIND_HOST}
Configuration: 
- DEBUG: {DEBUG}
- LOCAL CREDENTIALS: {LOCAL_CREDENTIALS} ({PODIMO_EMAIL})
- PODIMO_HOSTNAME: {PODIMO_HOSTNAME}
- PODIMO_BIND_HOST: {PODIMO_BIND_HOST}
- PODIMO_PROTOCOL: {PODIMO_PROTOCOL}
- PUBLIC_FEEDS: {PUBLIC_FEEDS}
- HTTP_PROXY: {HTTP_PROXY}
- ZENROWS_API: {ZENROWS_API}
- SCRAPER_API: {SCRAPER_API}
- CACHE_DIR: {CACHE_DIR}
- STORE_TOKENS_ON_DISK: {STORE_TOKENS_ON_DISK}
- TOKEN_CACHE_TIME: {TOKEN_CACHE_TIME} sec
- PODCAST_CACHE_TIME: {PODCAST_CACHE_TIME} sec
- HEAD_CACHE_TIME: {HEAD_CACHE_TIME} sec
- BLOCKING: {BLOCKED}
""")
    asyncio.run(main())
