// Variáveis, escopos, closures
var a = 1; let b = 2; const c = 3;
console.log(a + b + c, typeof a, typeof "x", typeof null, typeof undefined, typeof {}, typeof function(){});
function f(x, y = x * 2, ...r) { return [x, y, r.length, arguments.length]; }
console.log(JSON.stringify(f(1)), JSON.stringify(f(1, 2, 3, 4)), f.length, f.name);
var fns = []; for (let i = 0; i < 3; i++) fns.push(() => i); console.log(fns.map(g => g()).join());
var fv = []; for (var j = 0; j < 3; j++) fv.push(() => j); console.log(fv.map(g => g()).join());
function contador() { let n = 0; return { inc() { return ++n; }, get v() { return n; } }; }
var ct = contador(); ct.inc(); ct.inc(); console.log(ct.v);
console.log(hoisted(), typeof naoExiste); function hoisted() { return "içada"; }
try { tdz; let tdz = 1; } catch (e) { console.log(e.name); }
try { const k = 1; k = 2; } catch (e) { console.log(e.name, e.message); }
try { undefinedVar; } catch (e) { console.log(e.name, e.message); }
try { null.x; } catch (e) { console.log(e.name, e.message); }
try { (void 0).y = 1; } catch (e) { console.log(e.name, e.message); }
// Operadores
console.log(1 + "2", "3" * "4", 5 / 2, 7 % 3, -7 % 3, 2 ** 10, 2 ** -1, 1 / 0, -1 / 0, 0 / 0);
console.log(5 & 3, 5 | 3, 5 ^ 3, ~5, 1 << 31, -1 >> 1, -1 >>> 28, 2147483647 + 1, 4294967296 | 0);
console.log(1 == "1", 1 === "1", null == undefined, null === undefined, NaN == NaN, [] == false, "" == 0, "a" < "b", "10" < "9", 10 < 9, "10" < 9);
console.log(null ?? "d", 0 ?? "d", 0 || "d", 1 && 2, null?.x, ({a: {b: 1}}).a?.b, ({}).a?.b.c.d, ({}).f?.(), ({f() { return 1; }}).f?.());
var la = null; la ??= 5; var lb = 0; lb ||= 6; var lc = 1; lc &&= 7; console.log(la, lb, lc);
var o1 = {p: null}; o1.p ??= 1; o1.q ||= 2; o1.p &&= 3; console.log(JSON.stringify(o1));
var idx = 0; var arr1 = [1, 2]; arr1[idx++] += 10; console.log(idx, arr1.join());
var post = {n: 1}; var pv = post.n++; console.log(pv, post.n, ++post.n, post.n--, post.n);
var pa = [5]; var pi = pa[0]++; console.log(pi, pa[0], --pa[0]);
console.log(typeof typeof 1, void 0, !0, !!"", +"3", -"3", +true, +[], +{}, +null, +undefined);
console.log(0.1 + 0.2, 1e21, 1e-7, 123456789012345680000, 1.5e300, -0, [-0], 0 === -0, Object.is(0, -0), 100, 1.10);
console.log((255).toString(16), (255).toString(2), (0.5).toString(2), (-255).toString(36), 3.14159.toFixed(2), (1.005).toFixed(2), (0.5).toFixed(0), (2.5).toFixed(0), (1e21).toFixed(2));
console.log((123.456).toPrecision(4), (0.00001).toPrecision(1), (1234.5678).toExponential(2), (0).toFixed(1), (-1.5).toFixed(0));
console.log(parseInt("42px"), parseInt("0x1F"), parseInt("101", 2), parseInt("z", 36), parseFloat("3.14abc"), parseFloat(".5"), parseInt(""), Number(""), Number(" 12 "), Number("1e3"), Number("0x10"), Number("abc"), Number(null), Number(true), Number([5]), Number("Infinity"));
console.log(String(123), String(null), String(undefined), String(true), String([1, [2, 3]]), String({}), String(Symbol("s")), `${1 + 1}a${"b"}`, [1, 2] + "", {} + "");
// Rótulos e controle
outer: for (var i = 0; i < 3; i++) { for (var k = 0; k < 3; k++) { if (k == 1) continue outer; if (i == 2) break outer; console.log(i, k); } }
bloco: { console.log("dentro"); break bloco; }
var sw = x => { switch (x) { case 1: return "um"; case 2: case 3: return "dois-tres"; default: return "outro"; } };
console.log(sw(1), sw(2), sw(3), sw(4));
var sf = 0; switch (2) { case 1: sf += 1; case 2: sf += 2; case 3: sf += 3; break; case 4: sf += 4; } console.log(sf);
switch (9) { default: console.log("def"); case 1: console.log("cai"); }
var w = 0; while (true) { if (++w > 4) break; } do { w--; } while (w > 2); console.log(w);
for (var p in {x: 1, y: 2, 3: 3}) console.log(p);
for (var v of [1, 2]) console.log(v);
for (const ch of "aé😀") console.log(ch);
for (var q in [7, 8]) console.log(typeof q, q);
for (var q2 in "ab") console.log(q2);
class Pai1 { constructor() { this.m = 1; } } Pai1.prototype.h = 2; for (var z in new Pai1()) console.log(z);
// try/finally
function t1() { try { return "try"; } finally { console.log("fin1"); } }
function t2() { try { throw 1; } catch (e) { return "catch" + e; } finally { console.log("fin2"); } }
function t3() { for (var i = 0; i < 3; i++) { try { if (i == 1) continue; if (i == 2) break; console.log("c", i); } finally { console.log("f", i); } } return "fim"; }
function t4() { try { try { throw new Error("x"); } finally { console.log("inner"); } } catch (e) { return e.message; } }
function t5() { try { return 1; } finally { return 2; } }
function t6() { l: try { return 1; } finally { break l; } return 3; }
console.log(t1(), t2(), t3(), t4(), t5(), t6());
function t7() { for (const x of [1, 2, 3]) { try { return x; } finally { console.log("t7"); } } } console.log(t7());
// Desestruturação
var [d1, , d2 = 9, ...d3] = [1, 2, undefined, 4, 5]; console.log(d1, d2, d3.join());
var {e1, e2: e3, e4 = 5, ...e5} = {e1: 1, e2: 2, x: 3, y: 4}; console.log(e1, e3, e4, JSON.stringify(e5));
var {n: {m: nm}, a: [a0]} = {n: {m: 1}, a: [2]}; console.log(nm, a0);
var s1 = 1, s2 = 2; [s1, s2] = [s2, s1]; console.log(s1, s2);
var ob = {}; [ob.a, ob["b"]] = [1, 2]; ({c: ob.c} = {c: 3}); console.log(JSON.stringify(ob));
function pd({a, b = 2} = {a: 1}, [c1] = [3]) { return a + b + c1; } console.log(pd(), pd({a: 5}, [1]));
for (const [k, v] of Object.entries({a: 1})) console.log(k, v);
for (var {x: fx} of [{x: 1}]) console.log(fx);
var [g1, g2] = (function*() { yield 1; yield 2; yield 3; })(); console.log(g1, g2);
try { var {a: x1} = null; } catch (e) { console.log(e.name); }
var [c9, c8] = "hi"; console.log(c9, c8);
var chave = "din"; var {[chave]: dv} = {din: 7}; console.log(dv);
// Objetos
var nome = "k"; var lit = {nome, [nome + "2"]: 1, "a-b": 2, 3: 4, m() { return this.nome; }, get g() { return 5; }, set g(v) { this.s = v; }, __proto__: {herd: 1}};
lit.g = 9; console.log(lit.m(), lit.g, lit.s, lit.herd, Object.keys(lit).join(), lit[3], lit["3"]);
console.log({...{a: 1}, b: 2, ...null, ...[7]}, [...[1, 2], ..."ab", ...new Set([3])].join());
console.log("x" in {x: 1}, "y" in {x: 1}, 0 in [1], delete lit.nome, lit.nome, [] instanceof Array, [] instanceof Object, ({}) instanceof Array);
var der = Object.create({h: 1}); der.o = 2; console.log(der.h, der.hasOwnProperty("h"), Object.getPrototypeOf(der).h);
Object.defineProperty(der, "ro", {value: 1, enumerable: false}); der.ro = 2; console.log(der.ro, Object.keys(der).join(), JSON.stringify(Object.getOwnPropertyDescriptor(der, "ro")));
var fz = Object.freeze({a: 1}); fz.a = 2; fz.b = 3; console.log(fz.a, fz.b, Object.isFrozen(fz));
var fa = Object.freeze([1]); try { fa.push(2); } catch (e) { console.log(e.name); } fa[0] = 9; console.log(fa[0]);
console.log(Object.entries({a: 1, b: 2}).join(";"), Object.assign({}, {a: 1}, {b: 2}).b, Object.fromEntries([["a", 1]]).a, Object.getOwnPropertyNames("ab").join());
console.log(Object.keys({b: 1, 2: 1, a: 1, 1: 1}).join(), Object.prototype.toString.call([]), Object.prototype.toString.call(null), Object.prototype.toString.call(new Date(0)), Object.prototype.toString.call(/a/), Object.prototype.toString.call(function(){}), String(Math), `${JSON}`);
var gs = {get a() { return 1; }}; console.log(Object.getOwnPropertyDescriptor(gs, "a").set, typeof Object.getOwnPropertyDescriptor(gs, "a").get);
console.log(typeof globalThis, (function() { return this === globalThis; })(), (function() { "use strict"; return this; })());
// Funções
console.log((function() { return arguments.length; })(1, 2), (() => 1).name, (function nomeada() {}).name, (function() {}).name, (class A {}).name);
var fe = function fat(n) { return n <= 1 ? 1 : n * fat(n - 1); }; console.log(fe(5), fe.name);
var b1 = function(x, y) { return [this.v, x, y]; }.bind({v: 1}, 2); console.log(b1(3).join(), b1.name, b1.length);
console.log(Math.max.apply(null, [1, 3, 2]), [].concat.call([1], [2]).join(), (function() { return this.q; }).call({q: 7}));
function Ctor(v) { if (!new.target) return "sem new"; this.v = v; } Ctor.prototype.g = function() { return this.v; }; console.log(new Ctor(1).g(), Ctor(1), new Ctor(2) instanceof Ctor, new Ctor(1).constructor === Ctor);
function Ret() { return {r: 1}; } console.log(new Ret().r);
console.log((function(a, b = 1, c) {}).length, ((a, b) => {}).length, (function(...r) {}).length);
var arrow = { v: 1, f() { return [1].map(() => this.v)[0]; }, g() { return (() => arguments[0])(); } }; console.log(arrow.f(), arrow.g(5));
console.log(function() { return typeof arguments; }(), (function(a) { arguments[0]; return a; })(1), function(){ return Array.prototype.slice.call(arguments, 1).join(); }(1, 2, 3));
console.log(String(function x(a) { return a; }), String(class Z { m() {} }), String(() => 1));
console.log(new Function("a", "b", "return a + b")(1, 2), eval("1 + 1"), eval("var ev = 3; ev * 2"), ev);
function rec(n) { return n == 0 ? 0 : 1 + rec(n - 1); } console.log(rec(2000));
try { (function inf() { inf(); })(); } catch (e) { console.log(e.name); }
// Tags e templates
function tag(s, ...v) { return s.raw.join("|") + v.join(); } console.log(tag`a${1}\n${2}c`, String.raw`\n${1}`);
console.log(`multi
linha`, `é\x41`, `a${`b${`c`}`}`);
// ASI
var asi1 = 1
var asi2 = 2
;[asi1, asi2].forEach(x => x)
var asi3 = asi1
+ asi2
console.log(asi3)
function asiR() { return
 1 } console.log(asiR())
var re1 = 4 / 2 / 1; var re2 = /a\/b/.test("a/b"); console.log(re1, re2, 1 / 2 / 1);
if (true) /x/.test("x"); var obj = {}; obj.a = 1
