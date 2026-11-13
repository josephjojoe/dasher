// Concatenation, search and case mapping on strings, compared with what
// the engine gives without dasher's string patches (see PATCHES.md).
// See tests/runtime.rs, strings_are_the_same_however_they_are_built.
(function () {
  const out = [];
  let seed = 3;
  const rnd = (n) => (seed = (seed * 1103515245 + 12345) & 0x7fffffff) % n;
  let h = 0;
  const mix = (v) => {
    const s = typeof v === "string" ? v + "/" + v.length : JSON.stringify(v);
    for (let j = 0; j < s.length; j++) h = (h * 31 + s.charCodeAt(j)) | 0;
  };

  // `s += x` appends in place only when nothing else can see the string.
  let s = "ab";
  const alias = s;
  s += "c";
  let t = s;
  s += 1;
  s += "Ā";
  s = s + 2;
  let u = "x";
  for (let i = 0; i < 300; i++) u += i & 7;
  const keep = [];
  let v = "";
  for (let i = 0; i < 50; i++) {
    v += String.fromCharCode(97 + (i % 26));
    if (i % 10 == 0) keep.push(v);
  }
  let big = "";
  for (let i = 0; i < 3000; i++) big = big + "0123456789";
  out.push(alias, t, s, u.length, u.slice(-20), keep.join(","), big.length, big.slice(29990));

  // `s += a + b` appends both without making `a + b`.
  let f = "";
  for (let i = 0; i < 200; i++) f += "ab" + (i & 7);
  let g = "g";
  g += g + "x";
  g += "\u0100" + 1;
  g += "y" + g;
  let w = "\u0100";
  for (let i = 0; i < 20; i++) w += "c" + i;
  let n2 = "";
  for (let i = 0; i < 5; i++) n2 += i + "z";
  let o2 = "";
  o2 += "a" + { toString() { return "T"; } };
  o2 += "b" + 1.5;
  function fused() {
    let acc = "";
    return (x) => (acc += "<" + x);
  }
  const fz = fused();
  fz(1);
  fz("q");
  out.push(f.length, f.slice(0, 12), f.slice(-9), g, w, n2, o2, fz(2));

  // A closure variable, and a string that is a property key (an atom).
  function counter() {
    let acc = "";
    return (x) => (acc += x);
  }
  const add = counter();
  add("p");
  add(1);
  out.push(add("q"));
  const o = {};
  let k = "key" + rnd(10);
  o[k] = 1;
  delete o[k];
  k += "z";
  o[k] = 2;
  out.push(Object.keys(o).join(), k, o["key" + k.slice(3, 4) + "z"]);

  // `str + int` and `int + str`
  out.push("a" + 5, 5 + "a", "" + -2147483648, "Ā" + 7, 7 + "Ā", "x" + 1.5);

  // search, split, replace and case mapping
  const alpha = ["a", "b", "A", "Z", " ", "é", "É", "µ", "ß", "ÿ",
    "×", "÷", "Ā", "Σ", "İ", "😀", "\ud800", "x", ",", "ab"];
  const str = (n) => {
    let r = "";
    for (let i = rnd(n); i > 0; i--) r += alpha[rnd(alpha.length)];
    return r;
  };
  for (let i = 0; i < 3000; i++) {
    const a = str(12);
    const b = rnd(4) ? str(3) : a.slice(rnd(a.length + 1), rnd(a.length + 1));
    const pos = [undefined, -1, 0, 1, 3, 100, NaN, 2.5][rnd(8)];
    mix([a.indexOf(b, pos), a.lastIndexOf(b, pos), a.includes(b, pos), a.startsWith(b, pos),
      a.endsWith(b, pos), a.split(b), a.split(b, rnd(3)), a.replace(b, "[$&]"),
      a.replaceAll(b, "-"), a.toLowerCase(), a.toUpperCase(), a.trim(), a.split("")]);
  }
  out.push(h);
  out.push(["abc".split(), "".split(""), "".split("a"), "a,b,".split(","), "aaa".split("a"),
    "abc".indexOf(""), "abc".indexOf("", 9), "abc".lastIndexOf(""), "abc".lastIndexOf("c", -5),
    "ÿµß".toUpperCase(), "ÀÉ×Þ".toLowerCase(),
    "x".repeat(5000).split("x").length, "aĀb".split("Ā"), "a-b-c".split("-", 2),
    "aXbXc".replace("X", "$$"), "abc".replace("b", "$'$`"), "The Lazy Dog".toLowerCase()]);
  return out;
})()
