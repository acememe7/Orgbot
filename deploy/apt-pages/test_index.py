#!/usr/bin/env python3
"""Check the lightweight GitHub Pages root and its publication mapping."""

from __future__ import annotations

from html.parser import HTMLParser
from pathlib import Path
import shutil
import tempfile

ROOT = Path(__file__).resolve().parents[2]
PAGE = ROOT / "deploy/apt-pages/index.html"
WORKFLOW = ROOT / ".github/workflows/publish-apt-pages.yml"
PRODUCT = "https://orgbots.theaecestudio.com/"
APT_RELEASE = "https://acememe7.github.io/Orgbot/apt/dists/stable/Release"


class PageParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.tags: list[tuple[str, dict[str, str | None]]] = []
        self.headings: list[int] = []
        self.links: set[str] = set()
        self.scripts = 0
        self.meta: dict[tuple[str, str], str | None] = {}
        self.in_title = False
        self.title_text = ""

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        attributes = dict(attrs)
        self.tags.append((tag, attributes))
        if tag == "meta":
            key = attributes.get("property") or attributes.get("name")
            if key:
                self.meta[(str(key), "content")] = attributes.get("content")
        if tag in {"h1", "h2", "h3", "h4", "h5", "h6"}:
            self.headings.append(int(tag[1]))
        if tag == "title":
            self.in_title = True
        if tag == "a" and attributes.get("href"):
            self.links.add(str(attributes["href"]))
        if tag == "script":
            self.scripts += 1

    def handle_endtag(self, tag: str) -> None:
        if tag == "title":
            self.in_title = False

    def handle_data(self, data: str) -> None:
        if self.in_title:
            self.title_text += data


def check_page() -> None:
    parser = PageParser()
    parser.feed(PAGE.read_text(encoding="utf-8"))
    canonicals = [
        tag.get("href")
        for name, tag in parser.tags
        if name == "link" and tag.get("rel") == "canonical"
    ]
    refreshes = [
        tag.get("content")
        for name, tag in parser.tags
        if name == "meta" and tag.get("http-equiv", "").lower() == "refresh"
    ]
    assert parser.title_text == "OrgBots | Product site", parser.title_text
    assert canonicals == [PRODUCT], canonicals
    assert refreshes == [f"0; url={PRODUCT}"], refreshes
    assert parser.headings == [1], parser.headings
    assert parser.scripts == 0, "The forwarding page must not load JavaScript"
    assert parser.meta.get(("description", "content")), "Missing page description"
    assert parser.meta.get(("og:image", "content")), "Missing Open Graph image"
    assert parser.meta.get(("twitter:card", "content")) == "summary_large_image"
    assert PRODUCT in parser.links
    assert PRODUCT + "install/" in parser.links
    assert APT_RELEASE in parser.links


def check_workflow_mapping() -> None:
    workflow = WORKFLOW.read_text(encoding="utf-8")
    required = (
        "ref: apt\n          path: site",
        "ref: ${{ github.sha }}\n          path: source",
        'cp -a "$repo" "$GH_WORKSPACE/public/apt"',
        'install -m 0644 "$GH_WORKSPACE/source/deploy/apt-pages/index.html" "$GH_WORKSPACE/public/index.html"',
    )
    for fragment in required:
        assert fragment in workflow, f"workflow is missing publication mapping: {fragment}"


def check_artifact_layout() -> None:
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        apt = root / "site" / "apt"
        release = apt / "dists/stable/Release"
        package = apt / "pool/orgbots_2.0.0_amd64.deb"
        release.parent.mkdir(parents=True)
        package.parent.mkdir(parents=True)
        release.write_bytes(b"signed repository metadata fixture")
        package.write_bytes(b"package fixture")
        public = root / "public"
        public.mkdir()
        shutil.copytree(apt, public / "apt")
        shutil.copyfile(PAGE, public / "index.html")

        assert (public / "index.html").read_bytes() == PAGE.read_bytes()
        assert (public / "apt/dists/stable/Release").read_bytes() == release.read_bytes()
        assert (public / "apt/pool/orgbots_2.0.0_amd64.deb").read_bytes() == package.read_bytes()
        assert not (public / "apt/index.html").exists()


if __name__ == "__main__":
    check_page()
    check_workflow_mapping()
    check_artifact_layout()
    print("GitHub Pages root forwarding page, APT preservation, and publication mapping passed")
