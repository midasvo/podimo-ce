from podimo.cache import getCacheEntry, insertCacheEntry


def test_insert_then_retrieve_before_expiry():
    cache = {}
    insertCacheEntry("k", "v", timeout=60, cache=cache)
    assert getCacheEntry("k", cache) == "v"
    # Not deleted on a hit.
    assert "k" in cache


def test_expired_entry_returns_none_and_is_deleted_by_default():
    cache = {}
    insertCacheEntry("k", "v", timeout=-1, cache=cache)
    assert getCacheEntry("k", cache) is None
    assert "k" not in cache


def test_expired_entry_with_delete_false_returns_none_but_keeps_key():
    cache = {}
    insertCacheEntry("k", "v", timeout=-1, cache=cache)
    # Mirrors getHeadEntry's delete=False usage.
    assert getCacheEntry("k", cache, delete=False) is None
    assert "k" in cache


def test_missing_key_returns_none():
    cache = {}
    assert getCacheEntry("missing", cache) is None
