from main import _arg, split_username_region_locale


def test_arg_plain_key():
    assert _arg({"region": "nl"}, "region") == "nl"


def test_arg_amp_prefixed_key_fallback():
    # Regression test for the fork-local workaround: Audiobookshelf and similar
    # tools consume the HTML feed URL without decoding `&amp;`, so the param
    # arrives prefixed with `amp;`. _arg accepts either form.
    assert _arg({"amp;region": "nl"}, "region") == "nl"


def test_arg_returns_none_when_missing():
    assert _arg({}, "region") is None


def test_arg_plain_key_wins_when_both_present():
    assert _arg({"region": "nl", "amp;region": "de"}, "region") == "nl"


def test_split_username_region_locale_three_parts():
    assert split_username_region_locale("a@b.com,nl,nl-NL") == ("a@b.com", "nl", "nl-NL")


def test_split_username_region_locale_one_part_defaults():
    # Older feed URLs were generated without region/locale; the fork-local
    # default is nl / nl-NL instead of erroring.
    assert split_username_region_locale("a@b.com") == ("a@b.com", "nl", "nl-NL")


def test_split_username_region_locale_four_parts_falls_back_to_defaults():
    # Malformed (too many parts) should not crash; falls back to defaults
    # against s[0] (the email).
    assert split_username_region_locale("a@b.com,nl,nl-NL,extra") == ("a@b.com", "nl", "nl-NL")
