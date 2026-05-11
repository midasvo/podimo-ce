import re

import pytest

from podimo.utils import (
    is_correct_email_address,
    randomFlyerId,
    randomHexId,
    token_key,
)

HEX_CHARS = set("0123456789abcdef")


@pytest.mark.parametrize("n", [0, 1, 16, 64])
def test_randomHexId_length_and_alphabet(n):
    out = randomHexId(n)
    assert isinstance(out, str)
    assert len(out) == n
    assert set(out).issubset(HEX_CHARS)


def test_randomFlyerId_format():
    out = randomFlyerId()
    assert re.fullmatch(r"\d{13}-\d{13}", out), out


def test_token_key_deterministic_same_inputs():
    a = token_key("user@example.com", "hunter2")
    b = token_key("user@example.com", "hunter2")
    assert a == b


def test_token_key_distinct_for_different_inputs():
    base = token_key("user@example.com", "hunter2")
    assert token_key("other@example.com", "hunter2") != base
    assert token_key("user@example.com", "different") != base


def test_token_key_is_64_char_hex_sha256():
    key = token_key("user@example.com", "hunter2")
    assert len(key) == 64
    assert set(key).issubset(HEX_CHARS)


@pytest.mark.parametrize(
    "addr",
    ["a@b.com", "foo+tag@bar.co.uk"],
)
def test_is_correct_email_address_accepts_valid(addr):
    assert is_correct_email_address(addr) is True


@pytest.mark.parametrize(
    "addr",
    [
        "",
        "no-at-sign",
        # parseaddr quirk: a bare "@" doesn't parse as an address at all
        # (returns ('', '')), so this is False.
        "@",
        # parseaddr quirk: "a@" likewise fails to parse; returns ('', '').
        "a@",
    ],
)
def test_is_correct_email_address_rejects_clearly_invalid(addr):
    assert is_correct_email_address(addr) is False


def test_is_correct_email_address_parseaddr_quirk_at_b_is_accepted():
    # parseaddr quirk: "@b" actually parses to ('', '@b') — there's an "@"
    # in the address slot, so the current implementation returns True even
    # though "@b" is obviously not a valid email. Document this so a future
    # tightening of the check (e.g. a real regex) deliberately breaks this
    # test instead of silently changing behavior.
    assert is_correct_email_address("@b") is True
