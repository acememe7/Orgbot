#!/usr/bin/env python3
"""Check public static pages without a browser or third-party dependencies."""

import json
import base64
import hashlib
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1] / 'website'
ORIGIN = 'https://orgbots.theaecestudio.com'


class Page(HTMLParser):
    def __init__(self, path):
        super().__init__(convert_charrefs=True)
        self.path = path
        self.tags = []
        self.headings = []
        self.title = ''
        self.in_title = False
        self.schema = None
        self.schemas = []
        self.schema_hashes = []
        self.ids = set()
        self.feed(path.read_text())

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        self.tags.append((tag, attrs))
        if 'id' in attrs:
            assert attrs['id'] not in self.ids, f'{self.path}: duplicate id'
            self.ids.add(attrs['id'])
        if tag in ('h1', 'h2', 'h3', 'h4', 'h5', 'h6'):
            self.headings.append(int(tag[1]))
        if tag == 'title':
            self.in_title = True
        if tag == 'script' and attrs.get('type') == 'application/ld+json':
            self.schema = ''

    def handle_endtag(self, tag):
        if tag == 'title':
            self.in_title = False
        if tag == 'script' and self.schema is not None:
            self.schemas.append(json.loads(self.schema))
            self.schema_hashes.append(base64.b64encode(hashlib.sha256(self.schema.encode()).digest()).decode())
            self.schema = None

    def handle_data(self, data):
        if self.in_title:
            self.title += data
        if self.schema is not None:
            self.schema += data


def main():
    pages = {p.resolve(): Page(p) for p in ROOT.rglob('*.html')}
    assert len(pages) >= 6, 'Expected product, install, results, legal and 404 pages'
    titles, descriptions, canonicals = set(), set(), set()
    for path, page in pages.items():
        context = str(path.relative_to(ROOT))
        assert page.headings.count(1) == 1, f'{context}: expected one h1'
        assert page.headings[0] == 1, f'{context}: headings must start with h1'
        assert all(b <= a + 1 for a, b in zip(page.headings, page.headings[1:])), context
        assert page.title and page.title not in titles, f'{context}: unique title required'
        titles.add(page.title)
        metadata = {a.get('name', a.get('property')): a.get('content')
                    for t, a in page.tags if t == 'meta'}
        description = metadata.get('description')
        assert description and description not in descriptions, f'{context}: unique description required'
        descriptions.add(description)
        assert metadata.get('viewport'), f'{context}: missing viewport'
        assert all(metadata.get(k) for k in ('og:title', 'og:description', 'og:image', 'twitter:card')), context
        canonical = [a.get('href') for t, a in page.tags if t == 'link' and a.get('rel') == 'canonical']
        relative = path.relative_to(ROOT).as_posix().removesuffix('index.html')
        assert canonical == [f'{ORIGIN}/{relative}'], f'{context}: incorrect canonical {canonical}'
        canonicals.add(canonical[0])
        if context == 'index.html':
            assert page.schemas, f'{context}: missing structured data'
        if context != '404.html':
            assert 'noindex' not in (metadata.get('robots') or ''), context
        assert '\u2014' not in path.read_text(), f'{context}: em dash in website copy'
        for tag, attrs in page.tags:
            assert not any(k.startswith('on') for k in attrs), f'{context}: inline handler'
            assert 'style' not in attrs, f'{context}: inline style violates site CSP'
            if tag == 'img':
                assert 'alt' in attrs and attrs.get('width') and attrs.get('height'), f'{context}: image metadata'
            for key in ('href', 'src'):
                value = attrs.get(key)
                if not value:
                    continue
                target = urlsplit(value)
                assert target.scheme not in ('http', 'javascript'), f'{context}: unsafe URL {value}'
                if target.netloc or target.scheme:
                    continue
                local = (ROOT / unquote(target.path).lstrip('/') if target.path.startswith('/')
                         else path.parent / unquote(target.path)) if target.path else path
                if local.is_dir():
                    local /= 'index.html'
                assert local.is_file(), f'{context}: missing {value}'
                if target.fragment and local.resolve() in pages:
                    assert unquote(target.fragment) in pages[local.resolve()].ids, f'{context}: missing anchor {value}'
        image_url = urlsplit(metadata['og:image'])
        assert image_url.netloc == urlsplit(ORIGIN).netloc, f'{context}: social image host'
        assert (ROOT / image_url.path.lstrip('/')).is_file(), f'{context}: missing social image'
    sitemap = ET.parse(ROOT / 'sitemap.xml')
    urls = {e.text for e in sitemap.findall('.//{http://www.sitemaps.org/schemas/sitemap/0.9}loc')}
    assert urls == canonicals - {f'{ORIGIN}/404.html'}, 'Sitemap differs from public pages'
    assert f'Sitemap: {ORIGIN}/sitemap.xml' in (ROOT / 'robots.txt').read_text()
    for config in (ROOT.parent / 'deploy/website').glob('*.conf'):
        policy = config.read_text()
        for page in pages.values():
            for digest in page.schema_hashes:
                assert f"'sha256-{digest}'" in policy, f'{config.name}: stale JSON-LD hash'
    print(f'{len(pages)} pages: metadata, headings, structured data, links, assets and sitemap passed')


if __name__ == '__main__':
    main()
