// Coleta de lixo, laços e textos grandes
var s = 0; for (var i = 0; i < 300000; i++) { var o = {a: i, b: [i]}; s += o.b[0] % 3; } console.log(s);
var t = ""; for (var i = 0; i < 20000; i++) t += "x" + i; console.log(t.length, t.slice(-5));
var parts = []; for (var i = 0; i < 20000; i++) parts.push(String(i)); console.log(parts.join("").length);
function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); } console.log(fib(24));
var m = new Map(); for (var i = 0; i < 20000; i++) m.set("k" + i, i); var soma = 0; for (var [k, v] of m) soma += v; console.log(soma, m.size);
var ob = {}; for (var i = 0; i < 5000; i++) ob["p" + i] = i; for (var i = 0; i < 2500; i++) delete ob["p" + i * 2]; console.log(Object.keys(ob).length, ob.p1, ob.p2);
var closures = []; for (let i = 0; i < 10000; i++) closures.push(() => i); console.log(closures[9999](), closures.reduce((a, f) => a + f(), 0));
var nested = []; for (var i = 0; i < 2000; i++) nested = [nested]; var d = 0; while (nested.length) { nested = nested[0]; d++; } console.log(d);
function* g() { for (var i = 0; i < 20000; i++) yield i; } var c = 0; for (var x of g()) c += x; console.log(c);
class P { constructor(x) { this.x = x; } get d() { return this.x * 2; } } var acc = 0; for (var i = 0; i < 50000; i++) acc += new P(i).d % 7; console.log(acc);
var str = "abc".repeat(5000); console.log(str.replace(/b/g, "XX").length, str.split("a").length, str.match(/c/g).length);
var arr = []; for (var i = 0; i < 50000; i++) arr.push((i * 7919) % 10007); arr.sort((a, b) => a - b); console.log(arr[0], arr[49999], arr.filter(x => x % 2).map(x => x + 1).reduce((a, b) => a + b, 0));
var big = {}; big.self = big; big.list = []; for (var i = 0; i < 1000; i++) big.list.push({up: big, i}); big = null;
var json = JSON.stringify(Array.from({length: 2000}, (_, i) => ({id: i, n: "x" + i, t: [i, i + 1]}))); console.log(json.length, JSON.parse(json)[1999].t[1]);
(async () => { var s = 0; for (var i = 0; i < 2000; i++) s += await i; console.log("async", s); })();
