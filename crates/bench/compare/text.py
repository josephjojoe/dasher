"""Visible text of an HTML document, for comparing two browsers' output."""

import re
from html.parser import HTMLParser

SKIP = {"script", "style", "noscript", "template", "head", "svg", "iframe", "object"}
VOID = {"area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta",
        "param", "source", "track", "wbr"}


class _P(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.stack = []
        self.skip = 0
        self.out = []

    def handle_starttag(self, tag, attrs):
        if tag in VOID:
            return
        # Shadow trees serialized as declarative shadow DOM are content.
        skip = tag in SKIP and not (tag == "template" and any(
            k in ("shadowrootmode", "shadowroot") for k, _ in attrs))
        self.stack.append((tag, skip))
        if skip:
            self.skip += 1

    def handle_endtag(self, tag):
        for i in range(len(self.stack) - 1, -1, -1):
            if self.stack[i][0] == tag:
                for _, s in self.stack[i:]:
                    if s:
                        self.skip -= 1
                del self.stack[i:]
                return

    def handle_data(self, data):
        if not self.skip:
            self.out.append(data)


def visible_text(html):
    p = _P()
    try:
        p.feed(html)
        p.close()
    except Exception:  # noqa: BLE001 - keep what was parsed
        pass
    return " ".join(" ".join(p.out).split())


WORD = re.compile(r"\w+", re.UNICODE)


def words(text):
    """Lowercased words with digits masked."""
    return [re.sub(r"\d", "0", w.lower()) for w in WORD.findall(text)]
