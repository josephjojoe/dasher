// Every pattern below is run over every subject through each way a script
// can reach the regexp matcher. tests/regexp.rs runs this file with
// dasher's search loop and with the engine's own and expects the same
// answers. V8 gives the same answers too (checked through Lightpanda
// 1.0.3 when the file was written).
var subjects = ['', 'a', 'abc abc', 'Mozilla/5.0 (X11; Linux x86_64) Chrome/150.0.0.0 Safari/537.36', 'no-js home  blog\twp', 'aaa', 'xAbCx', 'line1\nline2\r\nline3', 'café naïve 中文 😀😀 end', 'a,b;c,,d', '2026-11-11T06:05:09Z', 'foo.bar[0].baz', 'aXbXc', '😀', 'x\ud83dy', 'The Quick brown fox', '   lead', 'tail   ', 'ab'.repeat(50) + 'c'];
var res = [/a/, /abc/g, /Chrome\/(\d+)/, /\bno-js\b/, /\s+/g, /(?:)/g, /x*/g, /a*?/g, /$/, /^/gm, /$/gm, /(a)|(b)/g, /(iphone|ipad|android|mobile|quick)/i, /[A-Z]/g, /\d{4}-(\d\d)-(\d\d)/, /(?<y>\d{4})-(?<m>\d\d)/, /(?<=a)b/g, /(?<!a)b/g, /b(?=c)/g, /./gu, /\ud83d/g, /\ud83d/gu, /\u{1F600}/gu, /[^\x00-\x7f]+/g, /\p{L}+/gu, /é/i, /É/i, /é/iu, /c$/, /(ab)+c/, /(ab)*?c/, /X/y, /a/y, /\./g, /[,;]/, /,/, /e/g, /E/gi, /\w+@\w+/, /^\s+|\s+$/g, /(\w)(\w)?/g, /Z/, /Z|$/g, /fox|brown/, /quick/i, /中文/, /文/u, /a{2,}/, /\n/g, /\r?\n/g, /l(?:ine)(\d)/g];
var T = {};
res.forEach(function (re) {
  var out = [];
  subjects.forEach(function (s) {
    var r = [];
    try {
      re.lastIndex = 0; var m = re.exec(s); r.push(m ? [m.index, re.lastIndex].concat(Array.prototype.slice.call(m)).concat(m.groups ? [JSON.stringify(m.groups)] : []) : null);
      re.lastIndex = 0; r.push(re.test(s));
      re.lastIndex = 2; r.push(re.test(s), re.lastIndex);
      re.lastIndex = 0; r.push(s.search(re));
      r.push(s.replace(re, '<$&>'));
      r.push(s.split(re).slice(0, 12));
      r.push((s.match(re) || []).slice(0, 12));
      if (re.global) { var all = []; for (var x of s.matchAll(re)) all.push(x.index + ':' + x[0]); r.push(all.slice(0, 12)); }
      re.lastIndex = 0;
    } catch (e) { r.push('THROW ' + e); }
    out.push(r);
  });
  T[String(re)] = out;
});
var out = JSON.stringify(T);
