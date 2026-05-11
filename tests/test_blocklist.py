from podimo.config import load_block_list


def test_missing_path_returns_empty_set(tmp_path):
    assert load_block_list(str(tmp_path / "does-not-exist")) == set()


def test_parses_ids_strips_comments_and_blank_lines(tmp_path):
    content = (
        "# a leading comment\n"
        "\n"
        "1234567890\n"
        "  abcdefghij  \n"
        "\n"
        "# another comment\n"
        "de9b2081-9fc5-489f-b9d3-d744ed9cab20 # inline description\n"
        "    # indented comment line\n"
    )
    # After strip(), the indented comment line begins with `#` and is
    # correctly skipped — verified by its absence from the expected set.
    p = tmp_path / ".block-list"
    p.write_text(content)

    result = load_block_list(str(p))
    assert result == {
        "1234567890",
        "abcdefghij",
        "de9b2081-9fc5-489f-b9d3-d744ed9cab20",
    }


def test_only_first_whitespace_token_is_kept(tmp_path):
    # Inline tail after a space is dropped.
    p = tmp_path / ".block-list"
    p.write_text("ABCDE description goes here\n")
    assert load_block_list(str(p)) == {"ABCDE"}


def test_empty_file_returns_empty_set(tmp_path):
    p = tmp_path / ".block-list"
    p.write_text("")
    assert load_block_list(str(p)) == set()
