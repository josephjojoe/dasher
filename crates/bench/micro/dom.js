// DOM micro-benchmarks: one DOM call in a fixed loop, on the generated
// page of about 9,000 nodes (`dom_page` in src/micro.rs): 500 cards of
// a heading, a paragraph and a list of four links, with `#target` (a div
// with classes, a data attribute, an inline style, a span and a paragraph)
// after the 250th card and `#last` at the very end.
//
// Each entry is `bench(name, iterations, fn)`; `fn(n)` makes the call `n`
// times. Keep the loop bodies fixed: changing one makes old numbers
// meaningless. The harness raises the iteration count for loops that take
// under 100 ms.

var MICRO = typeof MICRO === "undefined" ? [] : MICRO;
var sink;
function bench(name, n, fn) {
  MICRO.push({ suite: "dom", name: name, n: n, fn: fn });
}
function target() {
  return document.getElementById("target");
}

bench("getElementById", 2e3, function (n) {
  for (let i = 0; i < n; i++) sink = document.getElementById("last");
});

bench("getComputedStyle", 4e3, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) sink = getComputedStyle(el).display;
});

bench("classList.contains", 3e4, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) sink = el.classList.contains("b");
});

bench("style.setProperty", 3e4, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) el.style.setProperty("width", (i & 7) + "px");
});

bench("add+removeEventListener", 5e4, function (n) {
  const el = target();
  const h = function () {};
  for (let i = 0; i < n; i++) {
    el.addEventListener("click", h);
    el.removeEventListener("click", h);
  }
});

bench("clientWidth", 5e4, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) sink = el.clientWidth;
});

bench("append+removeChild", 1e5, function (n) {
  const el = target();
  const child = document.createElement("div");
  for (let i = 0; i < n; i++) {
    el.appendChild(child);
    el.removeChild(child);
  }
});

bench("dataset.x", 1e5, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) sink = el.dataset.x;
});

bench("setAttribute", 1e5, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) el.setAttribute("title", i & 1 ? "odd" : "even");
});

bench("attributes.length", 1e5, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) sink = el.attributes.length;
});

bench("getAttribute", 2e5, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) sink = el.getAttribute("class");
});

bench("matches(tag)", 2e5, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) sink = el.matches("div");
});

bench("querySelector(tag)", 2e5, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) sink = el.querySelector("span");
});

bench("tagName", 5e5, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) sink = el.tagName;
});

bench("nextSibling", 5e5, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) sink = el.nextSibling;
});

bench("nodeType", 1e6, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) sink = el.nodeType;
});

bench("shadowRoot", 1e6, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) sink = el.shadowRoot;
});

bench("children", 5e5, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) sink = el.children;
});

bench("innerText", 5e5, function (n) {
  const p = document.getElementById("para");
  for (let i = 0; i < n; i++) sink = p.innerText;
});

bench("createElement", 1e5, function (n) {
  for (let i = 0; i < n; i++) sink = document.createElement("div");
});

bench("document.cookie", 3e5, function (n) {
  document.cookie = "session=abc123";
  for (let i = 0; i < n; i++) sink = document.cookie;
});

bench("getBoundingClientRect", 5e4, function (n) {
  const el = target();
  for (let i = 0; i < n; i++) sink = el.getBoundingClientRect();
});

bench("querySelectorAll(#id)", 1e3, function (n) {
  for (let i = 0; i < n; i++) sink = document.querySelectorAll("#last").length;
});

bench("querySelectorAll(.class)", 1e3, function (n) {
  for (let i = 0; i < n; i++) sink = document.querySelectorAll(".text").length;
});

bench("querySelectorAll(tag)", 5e2, function (n) {
  for (let i = 0; i < n; i++) sink = document.querySelectorAll("li").length;
});
