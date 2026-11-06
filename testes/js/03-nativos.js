// Array
var a = [3, 1, 2];
console.log(a.slice().sort().join(), a.slice().sort((x, y) => y - x).join(), [10, 9, 1].sort().join(), ["b", undefined, "a"].sort().join(), a.map(x => x * 2).join(), a.filter(x => x > 1).join(), a.reduce((s, x) => s + x), a.reduceRight((s, x) => s + "" + x), a.indexOf(1), a.lastIndexOf(9), a.includes(2), [NaN].includes(NaN), [NaN].indexOf(NaN));
console.log(a.find(x => x > 1), a.findIndex(x => x > 5), a.findLast(x => x > 1), a.some(x => x > 2), a.every(x => x > 2), a.concat(4, [5, [6]]).length, a.join("-"), [1, [2, [3, [4]]]].flat().length, [1, [2, [3, [4]]]].flat(Infinity).join(), [1, 2].flatMap(x => [x, x]).join(), a.at(-1), a.slice(1).join(), a.slice(-2, -1).join());
var b = [1, 2, 3, 4, 5]; console.log(b.splice(1, 2).join(), b.join(), b.splice(1, 0, 9, 8), b.join(), b.splice(-1).join(), b.reverse().join(), b.shift(), b.unshift(7), b.pop(), b.push(0, 0), b.join(), b.fill(1, 1).join(), [1, 2, 3, 4].copyWithin(0, 2).join());
console.log(Array(3).length, Array(3).join("-"), Array(1, 2).length, [, 1].length, 0 in [, 1], Array.from("abc").join(), Array.from({length: 2}, (_, i) => i * 2).join(), Array.of(7).length, Array.isArray([]), Array.from(new Set([1, 1, 2])).length, Array.from(new Map([[1, 2]])).join());
var c = [1, 2]; c.length = 1; c[3] = 9; console.log(c.length, c.join(), Object.keys(c).join(), c.foo, c); c.foo = 1; console.log(Object.keys(c).join());
var h = [1, , 3]; var vis = []; h.forEach((x, i) => vis.push(i)); console.log(vis.join(), h.map(x => x * 2).length, h.filter(() => true).length, JSON.stringify(h));
console.log([...[1, 2].keys()].join(), [...[1, 2].values()].join(), [...[1, 2].entries()].join(";"), [1, 2, 3].toString(), [[1, 2], [3]].toString(), [null, undefined, 1].join(), [1,2,3].toReversed().join(), [3,1].toSorted().join(), [1,2].with(0, 9).join(), [1,2,3].toSpliced(0,1).join());
console.log(Array.prototype.map.call("ab", x => x + x).join(), Array.prototype.join.call({length: 2, 0: "a", 1: "b"}), Array.prototype.slice.call({length: 1, 0: "z"})[0]);
var st = [5, 1, 4].map((v, i) => ({v, i})).sort((x, y) => (x.v > 2) - (y.v > 2)); console.log(st.map(x => x.i).join());
var grande = []; for (var i = 0; i < 1000; i++) grande.push(1000 - i); grande.sort((x, y) => x - y); console.log(grande[0], grande[999]);
var ci = [1]; ci.push(ci); console.log(ci.join(), String(ci));
// String
var s = "Olá, Mundo";
console.log(s.length, s.toUpperCase(), s.toLowerCase(), s.charAt(2), s.charCodeAt(2), s[0], s.at(-1), s.indexOf("Mu"), s.lastIndexOf("o"), s.includes("á"), s.startsWith("Ol"), s.endsWith("do"), s.slice(5), s.slice(-5, -1), s.substring(5, 3), s.substr(-5, 2), s.split(", ").join("|"), s.repeat(2).length, s.padStart(12, "*"), s.padEnd(12, "ab"), "  x ".trim() + "|", " x ".trimStart() + "|", " x ".trimEnd() + "|", s.concat("!", 1), s.codePointAt(2));
console.log("😀".length, "😀".codePointAt(0), "😀".charCodeAt(0), String.fromCodePoint(128512), String.fromCharCode(72, 105), [..."😀a"].length, "a😀b".slice(3), "a-b-c".split("-", 2).join(), "abc".split("").join(), "".split("").length, "".split(",").length, "abc".split().length);
console.log("aXbXc".replace("X", "-"), "aXbXc".replaceAll("X", "-"), "abc".replace("b", "[$&]"), "abc".replace("b", "$`$'"), "abc".replace("b", () => "$&"), "aaa".replace(/a/g, (m, i) => i), "abc".replace(/(a)(b)/, "$2$1"), "x".replace(/(?<n>x)/, "[$<n>]"), "abc".replace(/z/, "y"), "a.b".replaceAll(".", "$$"));
console.log("b".localeCompare("a"), "a".localeCompare("b"), "a".localeCompare("a"), "ab" < "b", "Z" < "a", "é".normalize() === "é", "%E2".length, encodeURIComponent("a b&é/"), encodeURI("a b&é/"), decodeURIComponent("a%20b%C3%A9"), decodeURI("%20%26"), escape("a b+é"), unescape("%41%u00e9"));
console.log("abc".localeCompare("abd"), new String("x").length, typeof new String("x"), typeof String(1), new String("ab")[1], "abc".substring(2, 0), "abc".substr(1), "abc".slice(2, 1) === "", Object.keys(new String("ab")).join());
// Regex
console.log(/a(b)?c/.exec("xac").index, /a(b)?c/.exec("xac")[1], /(\d+)-(\d+)/.exec("10-20").slice(1).join(), "2026-11-06".match(/(?<y>\d+)-(?<m>\d+)/).groups.m, "a1b2".match(/\d/g).join(), "abc".match(/z/), "abc".match(/z/g), "aB".match(/b/i)[0], "x\ny".match(/^y$/m)[0], "x\ny".match(/x.y/s)[0], /\bfoo\b/.test("a foo b"), /\Bfoo/.test("afoo"));
var g = /a/g; console.log(g.test("aa"), g.lastIndex, g.test("aa"), g.lastIndex, g.test("aa"), g.lastIndex, /a/y.test("ba"), g.source, g.flags, g.global, String(/a\/b/gi), new RegExp("a/b").source, new RegExp(/x/g).flags, new RegExp(/x/g, "i").flags);
console.log("aaa".match(/a{2}/)[0], "aaaa".match(/a{1,2}?/)[0], "aaa".match(/a*?/)[0] === "", "abab".match(/(ab)+/)[0], "abc".match(/[^a]+/)[0], "a-b".match(/[a\-]+/)[0], "x]".match(/[\]]/)[0], /(a)\1/.test("aa"), /(?<q>a)\k<q>/.test("aa"), /a(?=b)/.exec("ab")[0], /a(?!b)/.test("ab"), /(?<=a)b/.exec("ab")[0], /(?<!a)b/.test("ab"), /[\d\s]+/.exec("a1 2b")[0], /\w+@\w+\.\w+/.exec("ir para eu@x.com.br")[0], /é/.test("é"), /\x41/.test("A"), /é/i.test("É"), /[à-ú]/.test("é"));
console.log([..."a1b2".matchAll(/[a-z](\d)/g)].map(m => m[1] + m.index).join(), "a,b;c".split(/[,;]/).join("|"), "abc".split(/(b)/).join("|"), "abc".split(/(?:)/).join("|"), "a1b2c3".split(/\d/).join("|"), "  a  b ".trim().split(/\s+/).join("|"), "abc".search(/c/), "abc".search("z"), /(z)|(a)/.exec("a").join("~"), /(a)|b/.exec("b")[1], /(?:a|b)+/.exec("abab")[0], /(a*)*/.exec("b")[0] === "", /(a*)+/.exec("aa")[0], /\p{L}+/u.exec("1aé2")[0], /^[\w.+-]+$/.test("a.b+c-d"), /\//.source);
console.log("x".replace(/x/, "$0"), "john smith".replace(/(\w+)\s(\w+)/, "$2, $1"), "camelCaseText".replace(/([A-Z])/g, " $1").toLowerCase(), "aaa".lastIndexOf("a"), "foo.bar".split(".").length, "a\tb".split(/\t/).length, "<b>x</b>".replace(/<[^>]+>/g, ""), "  ".replace(/\s/g, "_"), "ÁÉ".toLowerCase(), "ß".toUpperCase());
// Number, Math
console.log(Number.MAX_SAFE_INTEGER, Number.EPSILON > 0, Number.isInteger(5.0), Number.isInteger(5.5), Number.isSafeInteger(2 ** 53), Number.isNaN("x"), isNaN("x"), Number.isFinite("1"), isFinite("1"), Number.parseFloat("1.5"), (5).valueOf(), new Number(5) + 1, typeof new Number(5), Number.MIN_VALUE, Number.MAX_VALUE);
console.log(Math.floor(-1.5), Math.ceil(-1.5), Math.round(-1.5), Math.round(1.5), Math.round(2.5), Math.round(0.49999999999999994), Math.trunc(-1.9), Math.abs(-3), Math.sign(-3), Math.max(), Math.min(), Math.max(1, NaN), Math.min(0, -0), Math.pow(2, 8), Math.sqrt(16), Math.sqrt(2), Math.cbrt(27), Math.hypot(3, 4), Math.imul(3, 4), Math.clz32(1), Math.fround(5.5));
console.log(Math.sin(0), Math.cos(0), Math.PI, Math.E, Math.log(Math.E), Math.log10(1000), Math.log2(8), Math.exp(0), Math.atan2(1, 1).toFixed(10), Math.sin(1).toFixed(10), Math.cos(1).toFixed(10), Math.tan(1).toFixed(10), Math.atan(1).toFixed(10), Math.asin(1).toFixed(10), Math.acos(0.5).toFixed(10), Math.exp(1).toFixed(10), Math.log(10).toFixed(10), Math.pow(2, 0.5).toFixed(10), Math.pow(10, -2), Math.sinh(1).toFixed(8), Math.cosh(1).toFixed(8), Math.tanh(1).toFixed(8), Math.log1p(1).toFixed(10), Math.expm1(1).toFixed(10));
var r = Math.random(); console.log(r >= 0 && r < 1, 5 % 0, 5.5 % 2, -5.5 % 2, 1e308 * 10, 2 ** 1024, 2 ** 53 + 1, 0.1 * 3, 1 / 3, 100 / 3, 1e21 + 1, 1.7976931348623157e308, 5e-324, 123e-20, 0.000001, 0.0000001);
// JSON
console.log(JSON.stringify({a: [1, {b: 2}], c: "x\n\"", d: null, e: undefined, f() {}, g: NaN, h: new Date(0), i: true}), JSON.stringify([undefined, function() {}]), JSON.stringify("é"), JSON.stringify(1), JSON.stringify(null), JSON.stringify(undefined), JSON.stringify({a: 1, b: [1, 2]}, null, 2), JSON.stringify({a: 1, b: 2}, ["a"]), JSON.stringify({a: 1, b: 2}, (k, v) => typeof v == "number" ? v + 1 : v), JSON.stringify({toJSON() { return 5; }}), JSON.stringify([], null, 2), JSON.stringify({}, null, 2), JSON.stringify({a: []}, null, "--"));
console.log(JSON.parse('{"a":[1,2,{"b":null}],"c":"\\u00e9\\n","d":1.5e2,"e":true}').c, JSON.parse("[1]")[0], JSON.parse('"x"'), JSON.parse("1"), JSON.parse(" null "), JSON.parse('{"a":1,"b":2}', (k, v) => typeof v == "number" ? v * 2 : v).b, Object.keys(JSON.parse('{"b":1,"a":2,"1":3}')).join());
var circ = {}; circ.c = circ; try { JSON.stringify(circ); } catch (e) { console.log(e.name); }
for (var bad of ["{a:1}", "[1,]", "01", "'a'", "", "tru", '{"a"}']) { try { JSON.parse(bad); console.log("aceitou", bad); } catch (e) { console.log(e.name); } }
// Date (fuso UTC nos testes)
var d = new Date(Date.UTC(2026, 10, 6, 13, 5, 9, 7));
console.log(d.getTime(), d.toISOString(), d.getFullYear(), d.getMonth(), d.getDate(), d.getDay(), d.getHours(), d.getMinutes(), d.getSeconds(), d.getMilliseconds(), d.getTimezoneOffset(), JSON.stringify(d), d.toUTCString(), String(d), d.toDateString());
console.log(new Date("2026-11-06").getTime(), new Date("2026-11-06T10:00:00Z").getTime(), new Date("2026-11-06T10:00:00.5+02:00").getTime(), Date.parse("Fri, 06 Nov 2026 10:00:00 GMT"), new Date(2026, 0, 31).getMonth(), new Date(2026, 12, 1).getFullYear(), new Date(NaN).getTime(), String(new Date(NaN)), new Date(0).toISOString(), new Date(99, 0).getFullYear(), new Date(-1).toISOString(), typeof Date.now());
var d2 = new Date(0); d2.setFullYear(2000); d2.setMonth(1, 29); d2.setHours(25); d2.setMinutes(61); console.log(d2.toISOString(), d2 - new Date(0), d2 > new Date(0), +new Date(5), new Date(5) + "" === new Date(5).toString(), new Date(new Date(7)).getTime());
try { new Date(NaN).toISOString(); } catch (e) { console.log(e.name); }
// Map, Set, Weak
var m = new Map([[1, "a"], ["1", "b"]]); var ko = {}; m.set(ko, 3).set(NaN, 4).set(-0, 5);
console.log(m.size, m.get(1), m.get("1"), m.get(ko), m.get(NaN), m.get(0), m.has(2), m.delete(1), m.size, [...m.keys()].length, [...m.values()].join(), [...m.entries()][0].join(), Object.prototype.toString.call(m)); var vm = []; m.forEach((v, k) => vm.push(v)); console.log(vm.join());
var se = new Set([1, 1, "1", NaN, NaN]); se.add(2).add(2); console.log(se.size, se.has(NaN), se.delete(1), [...se].join(), [...se.entries()][0].join()); var mm = new Map([[1, 1], [2, 2]]); for (var [k] of mm) { if (k == 1) mm.delete(2); console.log(k); }
var wm = new WeakMap(); wm.set(ko, 1); console.log(wm.get(ko), wm.has({}), new WeakSet([ko]).has(ko), new WeakRef(ko).deref() === ko); try { wm.set(1, 1); } catch (e) { console.log(e.name); }
console.log(new Map(), new Set(), new Map([[1, {a: 1}]]), new Set(["a"]), [1, "a", null, undefined, true], {a: {b: {c: {d: 1}}}}, [[]], [{}], function f() {}, class K {}, Symbol("q"), /r/g, new Error("e").message, null, undefined, -0, "s", ["s"], {s: "s"}, {"a-b": 1}, [function() {}]);
// Typed
var u = new Uint8Array([1, 300, -1]); console.log(u[1], u[2], u.length, u.join(), new Uint8ClampedArray([300, -5, 1.5])[0], new Int16Array(2).length, new Float64Array([1.5])[0], new Uint32Array([-1])[0], Array.from(u).length, u.buffer.byteLength, new Uint8Array(new ArrayBuffer(4), 1, 2).length, u.subarray(1).length, u.slice(1).join(), Int8Array.BYTES_PER_ELEMENT, Object.prototype.toString.call(u));
var i32 = new Int32Array(new Uint8Array([1, 0, 0, 0, 0, 1, 0, 0]).buffer); console.log(i32[0], i32[1], [...i32].join(), u.map(x => x * 2).join(), u.indexOf(44)); i32[5] = 1; console.log(i32[5], i32.fill(7).join()); u.set([9], 1); console.log(u[1]);
// Promises e assíncronas: a ordem importa
console.log("sinc 1");
setTimeoutFalso(() => console.log("nunca"));
function setTimeoutFalso() {}
Promise.resolve(1).then(v => { console.log("then", v); return v + 1; }).then(v => console.log("then2", v));
new Promise((ok, no) => { console.log("exec"); no(new Error("x")); }).catch(e => { console.log("catch", e.message); return 5; }).finally(() => console.log("fin")).then(v => console.log("depois", v));
queueMicrotask(() => console.log("micro"));
async function af(x) { console.log("af ini"); var v = await x; console.log("af", v); try { await Promise.reject(new Error("rej")); } catch (e) { console.log("af catch", e.message); } finally { console.log("af fin"); } return v * 2; }
af(5).then(v => console.log("af fim", v));
var aa = async () => { throw new Error("arrow"); }; aa().catch(e => console.log(e.message));
Promise.all([1, Promise.resolve(2), af(3)]).then(v => console.log("all", v.join()));
Promise.allSettled([1, Promise.reject(2)]).then(v => console.log(JSON.stringify(v)));
Promise.race([new Promise(() => {}), 7]).then(v => console.log("race", v));
Promise.any([Promise.reject(1), 8]).then(v => console.log("any", v));
Promise.any([Promise.reject(1)]).catch(e => console.log(e.name, e.errors.join()));
var thenable = {then(ok) { console.log("thenable"); ok(9); }}; Promise.resolve(thenable).then(v => console.log("t", v));
(async () => { console.log(await thenable); })();
async function* ag() { yield 1; var x = await Promise.resolve(2); yield x; return 3; }
(async () => { for await (const v of ag()) console.log("for await", v); var it = ag(); console.log(JSON.stringify(await it.next()), JSON.stringify(await it.next()), JSON.stringify(await it.next()), JSON.stringify(await it.next())); })();
class AC { async m() { return this.v; } v = 3; static async s() { return await 4; } } new AC().m().then(console.log); AC.s().then(console.log);
var op = {async f() { return 1; }, async *g() { yield 1; }}; op.f().then(v => console.log("op", v));
console.log(Object.prototype.toString.call(Promise.resolve()), typeof Promise.withResolvers().resolve, af.constructor !== Function, Object.getPrototypeOf(async function(){}) !== Function.prototype);
console.log("sinc fim");
