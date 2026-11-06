class A { #p = 1; x = 2; static s = 3; static #sp = 4; constructor(v) { this.v = v; } m() { return "A.m"; } get g() { return this.#p; } set g(v) { this.#p = v; } static sm() { return "sm" + this.#sp; } #pm() { return "priv"; } cp() { return this.#pm(); } static { A.bloco = 1; } tem(o) { return #p in o; } }
class B extends A { y = this.x + 1; constructor() { super(5); this.z = 1; } m() { return "B>" + super.m(); } static sm() { return "B" + super.sm; } get g() { return super.g + 1; } }
var b = new B(); console.log(b.v, b.x, b.y, b.z, b.m(), b.g, A.s, A.sm(), B.s, b.cp(), A.bloco, Object.keys(b).join());
b.g = 1; console.log(b instanceof A, b instanceof B, Object.getPrototypeOf(B) === A, B.name, typeof B, b.constructor.name, new A().tem(new A()));
try { A(); } catch (e) { console.log(e.name, e.message); }
class C2 extends B {} console.log(new C2().m());
class SemSuper extends A { constructor() { try { this.q; } catch (e) { console.log(e.name); } super(); } } new SemSuper();
class Err extends Error { constructor(m) { super(m); this.name = "Err"; this.extra = 1; } } var e = new Err("oi"); console.log(e.message, e.name, e instanceof Error, e instanceof Err, String(e), e.extra, typeof e.stack);
class Arr extends Array { soma() { return this.reduce((a, b) => a + b, 0); } } var ar = new Arr(); ar.push(1, 2); console.log(ar.soma(), ar.length, Array.isArray(ar), ar instanceof Arr);
class MapaX extends Map { g(k) { return this.get(k); } } var mx = new MapaX([[1, 2]]); console.log(mx.g(1), mx.size);
class Comp { static [("a" + "b")]() { return 1; } [Symbol.iterator]() { return [1, 2][Symbol.iterator](); } get [`k${1}`]() { return 2; } } console.log(Comp.ab(), [...new Comp()].join(), new Comp().k1);
var Exp = class Nome { n() { return Nome.name; } }; console.log(new Exp().n(), Exp.name);
var obj = { __proto__: { m() { return "proto"; } }, m() { return "obj>" + super.m(); } }; console.log(obj.m());
class Camp { a = 1; b = () => this.a; static c = this.name; ["d" + 1] = 2; e; } var cc = new Camp(); console.log(cc.b(), Camp.c, cc.d1, "e" in cc, cc.e);
class Getter { static get v() { return "sv"; } } console.log(Getter.v);
class Ret extends A { constructor() { return {r: 1}; } } console.log(new Ret().r);
function Velha() {} Velha.prototype.ola = function() { return "ola"; }; class NovaDeVelha extends Velha { } console.log(new NovaDeVelha().ola());
console.log(new (class { constructor() { this.t = new.target.name; } })().t === "", new (class Q { constructor() { this.t = new.target.name; } })().t);
// Geradores
function* g1() { var x = yield 1; console.log("x", x); try { yield 2; } finally { console.log("fim g"); } return 3; }
var i1 = g1(); console.log(JSON.stringify(i1.next()), JSON.stringify(i1.next("v")), JSON.stringify(i1.return(9)), JSON.stringify(i1.next()));
function* g2() { yield* [1, 2]; var r = yield* g3(); yield r; } function* g3() { yield 3; return 4; } console.log([...g2()].join());
function* g4() { try { yield 1; } catch (e) { yield "pegou " + e; } } var i4 = g4(); i4.next(); console.log(i4.throw("erro").value);
var og = { *m() { yield this.v; }, v: 7 }; console.log(og.m().next().value);
class CG { *[Symbol.iterator]() { yield 1; yield 2; } } console.log(Array.from(new CG()).join());
function* inf() { let n = 0; while (true) yield n++; } for (const v of inf()) { if (v > 2) break; console.log(v); }
function* fech() { try { yield 1; yield 2; } finally { console.log("fechou"); } } for (const v of fech()) { break; }
var [pr] = fech();
function* fib() { let [a, b] = [0, 1]; for (;;) { yield a; [a, b] = [b, a + b]; } } var ff = fib(); var seq = []; for (var k = 0; k < 10; k++) seq.push(ff.next().value); console.log(seq.join());
console.log(Object.prototype.toString.call(g1()), typeof g1().next, g1()[Symbol.iterator]().next !== undefined);
var iter = { from: 1, to: 3, [Symbol.iterator]() { var c = this.from, t = this.to; return { next: () => c <= t ? { value: c++, done: false } : { value: undefined, done: true }, return() { console.log("ret"); return {}; } }; } };
console.log([...iter].join(), Math.max(...iter)); for (var x of iter) { if (x == 2) break; }
try { for (var x of iter) { throw 1; } } catch (e) { console.log("capt", e); }
try { for (var y of 5) {} } catch (e) { console.log(e.name, e.message); }
try { [...undefined]; } catch (e) { console.log(e.name); }
var genFn = function*(){}; console.log(Object.getPrototypeOf(genFn) !== Function.prototype);
// Símbolos
var s = Symbol("d"); var os = {[s]: 1, r: 2}; console.log(s.toString(), s.description, typeof s, os[s], Object.keys(os).join(), Object.getOwnPropertySymbols(os).length, Symbol.for("q") === Symbol.for("q"), Symbol.keyFor(Symbol.for("q")));
try { s + ""; } catch (e) { console.log(e.name); }
var prim = {[Symbol.toPrimitive](h) { return h == "number" ? 1 : h == "string" ? "s" : "d"; }}; console.log(+prim, `${prim}`, prim + "");
var tagged = {get [Symbol.toStringTag]() { return "Tg"; }}; console.log(String(tagged));
class Inst { static [Symbol.hasInstance](v) { return v === 1; } } console.log(1 instanceof Inst, 2 instanceof Inst);
var vo = {valueOf() { return 5; }, toString() { return "t"; }}; console.log(vo + 1, `${vo}`, vo * 2, vo > 4, String(vo), [vo] + "");
// Getters, Reflect, Proxy
var px = new Proxy({a: 1}, { get(t, k, r) { return k in t ? t[k] : "faltou:" + String(k); }, set(t, k, v) { t[k] = v * 2; return true; }, has(t, k) { return k == "z" || k in t; }, deleteProperty(t, k) { console.log("del", k); delete t[k]; return true; }, ownKeys(t) { return ["a", "b"]; }, getOwnPropertyDescriptor(t, k) { return {value: 1, enumerable: true, configurable: true}; } });
console.log(px.a, px.b, "z" in px, "y" in px); px.c = 2; console.log(px.c); delete px.c; console.log(Reflect.ownKeys(px).join());
var pf = new Proxy(function(a) { return a + 1; }, { apply(t, th, ar) { return t(...ar) * 2; }, construct(t, ar) { return {c: ar[0]}; } }); console.log(pf(1), new pf(3).c, typeof pf);
console.log(Reflect.has({a: 1}, "a"), Reflect.get({a: 1}, "a"), Reflect.ownKeys({a: 1, [Symbol.iterator]: 2}).length, Reflect.apply(Math.min, null, [1, 2]), Reflect.construct(Date, [0]).getTime(), Reflect.getPrototypeOf([]) === Array.prototype);
var rec = {get v() { return this.w; }, w: 1}; console.log(Reflect.get(rec, "v", {w: 2}));
// Erros
var er = new TypeError("m"); console.log(er.name, er.message, er instanceof TypeError, er instanceof Error, String(er), Object.prototype.toString.call(er));
console.log(new Error("c", {cause: 5}).cause, new RangeError().message === "", Error("sem new").message, new AggregateError([1], "x").errors.length, typeof Error.captureStackTrace);
try { throw {a: 1}; } catch ({a}) { console.log(a); }
try { throw 1; } catch { console.log("sem param"); }
try { JSON.parse("{"); } catch (e) { console.log(e.name); }
try { new Array(-1); } catch (e) { console.log(e.name, e.message); }
try { [].reduce((a, b) => a); } catch (e) { console.log(e.name, e.message); }
try { null(); } catch (e) { console.log(e.name); }
try { var u; u.x(); } catch (e) { console.log(e.name); }
try { ({}).f(); } catch (e) { console.log(e.name); }
try { new (function() {}.bind())(); console.log("ok bind new"); new (() => 1)(); } catch (e) { console.log(e.name); }
try { eval("1 +"); } catch (e) { console.log(e.name); }
try { new RegExp("("); } catch (e) { console.log(e.name); }
try { "a".repeat(-1); } catch (e) { console.log(e.name); }
try { (1).toFixed(101); } catch (e) { console.log(e.name); }
try { decodeURIComponent("%"); } catch (e) { console.log(e.name); }
try { x = y_nao_existe; } catch (e) { console.log(e.message); }
(async () => { var r = []; for await (const v of [Promise.resolve(1), 2]) r.push(v); console.log("for await de array", r.join()); })();
