// As APIs da web que são só lógica, por cima das nativas de `web.rs` (`n`).
// Este arquivo é a expressão de uma função: roda uma vez, em cada página.
(function (n) {
"use strict";
const g = globalThis;
const oculto = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true, enumerable: false });
const pub = (k, v) => oculto(g, k, v);

// ------------------------------------------------------------- eventos
class Event {
  constructor(tipo, ini) {
    if (arguments.length < 1) throw new TypeError("Failed to construct 'Event': 1 argument required");
    ini = ini || {};
    this.type = String(tipo);
    this.bubbles = !!ini.bubbles;
    this.cancelable = !!ini.cancelable;
    this.composed = !!ini.composed;
    this.defaultPrevented = false;
    this.target = null;
    this.currentTarget = null;
    this.eventPhase = 0;
    this.timeStamp = Date.now();
    this.isTrusted = false;
    oculto(this, "__parar", false);
    oculto(this, "__caminho", []);
  }
  get srcElement() { return this.target; }
  get returnValue() { return !this.defaultPrevented; }
  set returnValue(v) { if (v === false) this.preventDefault(); }
  get cancelBubble() { return this.__parar; }
  set cancelBubble(v) { if (v) this.__parar = true; }
  preventDefault() { if (this.cancelable) this.defaultPrevented = true; }
  stopPropagation() { this.__parar = true; }
  stopImmediatePropagation() { this.__parar = true; }
  composedPath() { return this.__caminho.slice(); }
  initEvent(t, b, c) { this.type = t; this.bubbles = !!b; this.cancelable = !!c; }
}
Event.NONE = 0; Event.CAPTURING_PHASE = 1; Event.AT_TARGET = 2; Event.BUBBLING_PHASE = 3;
pub("Event", Event);
const copiar = (o, ini, campos) => { for (const k in campos) o[k] = ini && ini[k] !== undefined ? ini[k] : campos[k]; };
class CustomEvent extends Event { constructor(t, i) { super(t, i); this.detail = i && i.detail !== undefined ? i.detail : null; } }
class UIEvent extends Event { constructor(t, i) { super(t, i); copiar(this, i, { view: null, detail: 0 }); } }
class MouseEvent extends UIEvent { constructor(t, i) { super(t, i); copiar(this, i, { clientX: 0, clientY: 0, pageX: 0, pageY: 0, screenX: 0, screenY: 0, offsetX: 0, offsetY: 0, button: 0, buttons: 0, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, relatedTarget: null }); } }
class KeyboardEvent extends UIEvent { constructor(t, i) { super(t, i); copiar(this, i, { key: "", code: "", keyCode: 0, which: 0, charCode: 0, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, repeat: false, location: 0, isComposing: false }); } }
class FocusEvent extends UIEvent { constructor(t, i) { super(t, i); copiar(this, i, { relatedTarget: null }); } }
class WheelEvent extends MouseEvent { constructor(t, i) { super(t, i); copiar(this, i, { deltaX: 0, deltaY: 0, deltaZ: 0, deltaMode: 0 }); } }
class InputEvent extends UIEvent { constructor(t, i) { super(t, i); copiar(this, i, { data: null, inputType: "" }); } }
class PointerEvent extends MouseEvent { constructor(t, i) { super(t, i); copiar(this, i, { pointerId: 0, pointerType: "mouse", isPrimary: true }); } }
class ProgressEvent extends Event { constructor(t, i) { super(t, i); copiar(this, i, { loaded: 0, total: 0, lengthComputable: false }); } }
class ErrorEvent extends Event { constructor(t, i) { super(t, i); copiar(this, i, { message: "", filename: "", lineno: 0, colno: 0, error: null }); } }
class HashChangeEvent extends Event { constructor(t, i) { super(t, i); copiar(this, i, { oldURL: "", newURL: "" }); } }
class PopStateEvent extends Event { constructor(t, i) { super(t, i); this.state = i && i.state !== undefined ? i.state : null; } }
for (const c of [CustomEvent, UIEvent, MouseEvent, KeyboardEvent, FocusEvent, WheelEvent, InputEvent, PointerEvent, ProgressEvent, ErrorEvent, HashChangeEvent, PopStateEvent]) pub(c.name, c);
document.createEvent = function (t) { const e = new Event(""); e.initCustomEvent = function (a, b, c, d) { this.type = a; this.bubbles = b; this.cancelable = c; this.detail = d; }; return e; };

// Só os ouvintes por `on*` e os métodos: `g.addEventListener` já vem do protótipo.
pub("window", g); pub("self", g); pub("top", g); pub("parent", g); pub("frames", g);
pub("length", 0); pub("frameElement", null); pub("opener", null); pub("closed", false);
pub("name", ""); pub("status", ""); pub("origin", "");
Object.defineProperty(document, "defaultView", { value: g, configurable: true });

// ------------------------------------------------------------- classList
class DOMTokenList {
  constructor(el, attr) { oculto(this, "el", el); oculto(this, "attr", attr); }
  get __l() { const v = this.el.getAttribute(this.attr); return v ? v.trim().split(/\s+/).filter(Boolean) : []; }
  __g(l) { this.el.setAttribute(this.attr, l.join(" ")); }
  get length() { return this.__l.length; }
  get value() { return this.el.getAttribute(this.attr) || ""; }
  set value(v) { this.el.setAttribute(this.attr, v); }
  item(i) { const l = this.__l; return i < l.length ? l[i] : null; }
  contains(c) { return this.__l.includes(String(c)); }
  add(...cs) { const l = this.__l; for (const c of cs) { const s = String(c); if (!s || /\s/.test(s)) throw new DOMException("invalid token", "InvalidCharacterError"); if (!l.includes(s)) l.push(s); } this.__g(l); }
  remove(...cs) { this.__g(this.__l.filter(x => !cs.map(String).includes(x))); }
  toggle(c, f) { c = String(c); const t = this.contains(c); if (t && f !== true) { this.remove(c); return false; } if (!t && f !== false) { this.add(c); return true; } return t; }
  replace(a, b) { const l = this.__l; const i = l.indexOf(a); if (i < 0) return false; l[i] = b; this.__g(l); return true; }
  forEach(f, t) { this.__l.forEach((x, i) => f.call(t, x, i, this)); }
  toString() { return this.value; }
  [Symbol.iterator]() { return this.__l[Symbol.iterator](); }
  keys() { return this.__l.keys(); } values() { return this.__l.values(); } entries() { return this.__l.entries(); }
}
pub("DOMTokenList", DOMTokenList);
const tokens = new WeakMap();
Object.defineProperty(Element.prototype, "classList", { configurable: true, enumerable: true,
  get() { let l = tokens.get(this); if (!l) { l = new Proxy(new DOMTokenList(this, "class"), { get(t, k) { if (typeof k === "string" && /^\d+$/.test(k)) return t.item(+k); const v = t[k]; return typeof v === "function" ? v.bind(t) : v; } }); tokens.set(this, l); } return l; },
  set(v) { this.className = v; } });
Object.defineProperty(HTMLAnchorElement.prototype, "relList", { configurable: true, get() { return new DOMTokenList(this, "rel"); } });
class DOMException extends Error { constructor(m, nome) { super(m); this.name = nome || "Error"; this.code = 0; } }
pub("DOMException", DOMException);

// ------------------------------------------------------------- URL
const codif = s => encodeURIComponent(s).replace(/%20/g, "+").replace(/[!'()~]/g, c => "%" + c.charCodeAt(0).toString(16).toUpperCase());
const decod = s => { try { return decodeURIComponent(s.replace(/\+/g, " ")); } catch (e) { return s; } };
class URLSearchParams {
  constructor(i) {
    oculto(this, "__p", []); oculto(this, "__u", null);
    if (typeof i === "string") { if (i[0] === "?") i = i.slice(1); for (const par of i.split("&")) { if (!par) continue; const k = par.indexOf("="); this.__p.push(k < 0 ? [decod(par), ""] : [decod(par.slice(0, k)), decod(par.slice(k + 1))]); } }
    else if (i && typeof i[Symbol.iterator] === "function") { for (const [a, b] of i) this.__p.push([String(a), String(b)]); }
    else if (i) { for (const k of Object.keys(i)) this.__p.push([k, String(i[k])]); }
  }
  __m() { if (this.__u) this.__u.__busca = this.__p.length ? "?" + this.toString() : ""; }
  get size() { return this.__p.length; }
  append(a, b) { this.__p.push([String(a), String(b)]); this.__m(); }
  delete(a) { this.__p = this.__p.filter(x => x[0] !== a); this.__m(); }
  get(a) { const x = this.__p.find(y => y[0] === a); return x ? x[1] : null; }
  getAll(a) { return this.__p.filter(y => y[0] === a).map(y => y[1]); }
  has(a) { return this.__p.some(y => y[0] === a); }
  set(a, b) { const x = this.__p.find(y => y[0] === a); if (x) { x[1] = String(b); let achou = false; this.__p = this.__p.filter(y => { if (y[0] !== a) return true; if (achou) return false; achou = true; return true; }); } else this.__p.push([String(a), String(b)]); this.__m(); }
  sort() { this.__p.sort((x, y) => x[0] < y[0] ? -1 : x[0] > y[0] ? 1 : 0); this.__m(); }
  forEach(f, t) { for (const [a, b] of this.__p) f.call(t, b, a, this); }
  keys() { return this.__p.map(x => x[0])[Symbol.iterator](); }
  values() { return this.__p.map(x => x[1])[Symbol.iterator](); }
  entries() { return this.__p.map(x => [x[0], x[1]])[Symbol.iterator](); }
  [Symbol.iterator]() { return this.entries(); }
  toString() { return this.__p.map(([a, b]) => codif(a) + "=" + codif(b)).join("&"); }
}
pub("URLSearchParams", URLSearchParams);
const lerUrl = t => {
  const m = /^([a-zA-Z][a-zA-Z0-9+.-]*:)(?:\/\/(?:([^\/?#@]*)@)?([^\/?#:]*)(?::(\d*))?)?([^?#]*)(\?[^#]*)?(#.*)?$/.exec(t);
  if (!m) return null;
  m[1] = m[1].toLowerCase();
  const especial = /^(https?|ftp|file|wss?):$/.test(m[1]);
  let [u, s] = (m[2] || ":").split(":");
  return { protocolo: m[1], usuario: u || "", senha: s || "", host: (m[3] || "").toLowerCase(), porta: m[4] || "", caminho: m[5] || (especial ? "/" : ""), busca: m[6] && m[6] !== "?" ? m[6] : "", marca: m[7] && m[7] !== "#" ? m[7] : "", especial };
};
class URL {
  constructor(u, b) {
    u = String(u);
    let abs = /^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(u) ? u : null;
    if (!abs) {
      if (b === undefined) throw new TypeError("Failed to construct 'URL': Invalid URL");
      abs = n.resolver(u, String(new URL(b)));
      if (abs === null) throw new TypeError("Failed to construct 'URL': Invalid URL");
    }
    const p = lerUrl(abs);
    if (!p) throw new TypeError("Failed to construct 'URL': Invalid URL");
    // Tira os `.` e `..` do caminho.
    if (p.especial) {
      const saida = [];
      const partes = p.caminho.split("/");
      partes.forEach((q, i) => { if (q === "..") { if (saida.length > 1) saida.pop(); if (i === partes.length - 1) saida.push(""); } else if (q === ".") { if (i === partes.length - 1) saida.push(""); } else saida.push(q); });
      p.caminho = saida.join("/") || "/";
    }
    if ((p.protocolo === "http:" && p.porta === "80") || (p.protocolo === "https:" && p.porta === "443")) p.porta = "";
    oculto(this, "__x", p);
    oculto(this, "__params", null);
  }
  get protocol() { return this.__x.protocolo; } set protocol(v) { this.__x.protocolo = v.endsWith(":") ? v : v + ":"; }
  get username() { return this.__x.usuario; } get password() { return this.__x.senha; }
  get hostname() { return this.__x.host; } set hostname(v) { this.__x.host = v; }
  get port() { return this.__x.porta; } set port(v) { this.__x.porta = String(v); }
  get host() { return this.__x.host + (this.__x.porta ? ":" + this.__x.porta : ""); }
  set host(v) { const [a, b] = String(v).split(":"); this.__x.host = a; this.__x.porta = b || ""; }
  get pathname() { return this.__x.caminho; } set pathname(v) { v = String(v); this.__x.caminho = v[0] === "/" ? v : "/" + v; }
  get search() { return this.__x.busca; }
  set search(v) { v = String(v); this.__x.busca = v && v !== "?" ? (v[0] === "?" ? v : "?" + v) : ""; this.__params = null; }
  set __busca(v) { this.__x.busca = v; }
  get hash() { return this.__x.marca; } set hash(v) { v = String(v); this.__x.marca = v && v !== "#" ? (v[0] === "#" ? v : "#" + v) : ""; }
  get origin() { return this.__x.especial && this.__x.protocolo !== "file:" ? this.__x.protocolo + "//" + this.host : "null"; }
  get searchParams() { if (!this.__params) { this.__params = new URLSearchParams(this.__x.busca); this.__params.__u = this; } return this.__params; }
  get href() { const x = this.__x; const cred = x.usuario ? x.usuario + (x.senha ? ":" + x.senha : "") + "@" : ""; return x.protocolo + (x.especial || x.host ? "//" + cred + this.host : "") + x.caminho + x.busca + x.marca; }
  set href(v) { const o = new URL(v); this.__x = o.__x; this.__params = null; }
  toString() { return this.href; } toJSON() { return this.href; }
  static canParse(u, b) { try { new URL(u, b); return true; } catch (e) { return false; } }
  static createObjectURL() { return "blob:null"; } static revokeObjectURL() {}
}
pub("URL", URL);
// As partes do endereço nos <a>.
for (const k of ["protocol", "host", "hostname", "port", "pathname", "search", "hash", "origin", "username", "password"]) {
  Object.defineProperty(HTMLAnchorElement.prototype, k, { configurable: true,
    get() { try { return new URL(this.href)[k]; } catch (e) { return ""; } },
    set(v) { const u = new URL(this.href); u[k] = v; this.href = u.href; } });
}

// ------------------------------------------------------------- location e history
const loc = {
  get href() { return n.url(); }, set href(v) { n.navegar(String(v), false); },
  get protocol() { return new URL(n.url()).protocol; }, get host() { return new URL(n.url()).host; },
  get hostname() { return new URL(n.url()).hostname; }, get port() { return new URL(n.url()).port; },
  get pathname() { return new URL(n.url()).pathname; }, set pathname(v) { const u = new URL(n.url()); u.pathname = v; n.navegar(u.href, false); },
  get search() { return new URL(n.url()).search; }, set search(v) { const u = new URL(n.url()); u.search = v; n.navegar(u.href, false); },
  get hash() { return new URL(n.url()).hash; },
  set hash(v) { const u = new URL(n.url()); const velho = u.href; u.hash = v; if (u.href !== velho) { n.mudarEndereco(u.href, false); const a = u.hash.slice(1); const e = document.getElementById(a); if (e) e.scrollIntoView(); g.dispatchEvent(new HashChangeEvent("hashchange", { oldURL: velho, newURL: u.href })); } },
  get origin() { return new URL(n.url()).origin; },
  assign(u) { n.navegar(String(u), false); }, replace(u) { n.navegar(String(u), true); }, reload() { n.navegar(n.url(), true); },
  toString() { return n.url(); },
};
oculto(loc, Symbol.toStringTag, "Location");
Object.defineProperty(g, "location", { configurable: true, enumerable: true, get() { return loc; }, set(v) { n.navegar(String(v), false); } });
Object.defineProperty(document, "location", { configurable: true, get() { return loc; }, set(v) { n.navegar(String(v), false); } });
Object.defineProperty(document, "domain", { configurable: true, get() { return loc.hostname; }, set(v) {} });
let estadoHist = null;
const hist = {
  get length() { return 1; }, get state() { return estadoHist; }, scrollRestoration: "auto",
  pushState(s, t, u) { estadoHist = s === undefined ? null : s; if (u !== undefined && u !== null) { const d = new URL(u, n.url()); if (d.origin !== loc.origin) throw new DOMException("cross-origin pushState", "SecurityError"); n.mudarEndereco(d.href, false); } },
  replaceState(s, t, u) { estadoHist = s === undefined ? null : s; if (u !== undefined && u !== null) { const d = new URL(u, n.url()); if (d.origin !== loc.origin) throw new DOMException("cross-origin replaceState", "SecurityError"); n.mudarEndereco(d.href, true); } },
  back() { n.voltar(-1); }, forward() { n.voltar(1); }, go(d) { n.voltar(d || 0); },
};
pub("history", hist);

// ------------------------------------------------------------- janela
const def = (k, get) => Object.defineProperty(g, k, { configurable: true, enumerable: true, get });
def("innerWidth", () => n.janela()[0]); def("innerHeight", () => n.janela()[1]);
def("outerWidth", () => n.janela()[0]); def("outerHeight", () => n.janela()[1]);
def("scrollX", () => 0); def("pageXOffset", () => 0);
def("scrollY", () => n.rolagem()); def("pageYOffset", () => n.rolagem());
pub("devicePixelRatio", 1); pub("screenX", 0); pub("screenY", 0); pub("screenLeft", 0); pub("screenTop", 0);
const rolarJ = (a, b) => { const y = typeof a === "object" && a ? a.top : b; if (y !== undefined) n.rolar(y); };
pub("scrollTo", rolarJ); pub("scroll", rolarJ);
pub("scrollBy", (a, b) => { const y = typeof a === "object" && a ? a.top : b; n.rolar(n.rolagem() + (y || 0)); });
pub("alert", m => { n.avisar(String(m === undefined ? "" : m)); });
pub("confirm", m => { n.avisar(String(m === undefined ? "" : m)); return false; });
pub("prompt", m => { n.avisar(String(m === undefined ? "" : m)); return null; });
pub("print", () => {}); pub("focus", () => {}); pub("blur", () => {}); pub("stop", () => {}); pub("close", () => {});
pub("moveTo", () => {}); pub("resizeTo", () => {}); pub("getSelection", () => ({ rangeCount: 0, toString: () => "", removeAllRanges() {}, addRange() {}, getRangeAt() { return null; }, isCollapsed: true, type: "None" }));
pub("open", (u) => { if (u) n.navegar(String(u), false); return null; });
pub("postMessage", (m) => { setTimeout(() => { const e = new Event("message"); e.data = m; e.origin = loc.origin; e.source = g; g.dispatchEvent(e); }, 0); });
pub("getComputedStyle", (el) => {
  if (!(el instanceof Element)) throw new TypeError("Failed to execute 'getComputedStyle': parameter 1 is not of type 'Element'.");
  const o = n.estilo(el) || {};
  const kebab = s => s.replace(/-([a-z])/g, (_, c) => c.toUpperCase());
  o.getPropertyValue = p => { const v = o[kebab(p)]; return v === undefined ? "" : v; };
  o.getPropertyPriority = () => "";
  return o;
});
pub("matchMedia", q => {
  const ouv = [];
  return { media: String(q), get matches() { return n.midia(String(q)); }, onchange: null, addListener(f) { ouv.push(f); }, removeListener() {}, addEventListener() {}, removeEventListener() {}, dispatchEvent() { return true; } };
});
pub("screen", { width: 1024, height: 768, availWidth: 1024, availHeight: 768, colorDepth: 24, pixelDepth: 24, orientation: { type: "landscape-primary", angle: 0, addEventListener() {} } });
pub("navigator", {
  userAgent: "Mozilla/5.0 (FerrugemOS; x86_64) Ferrugem/0.2", appName: "Netscape", appVersion: "5.0 (FerrugemOS)", appCodeName: "Mozilla", product: "Gecko", vendor: "", platform: "FerrugemOS x86_64",
  language: "pt-BR", languages: ["pt-BR", "pt", "en"], onLine: true, cookieEnabled: true, hardwareConcurrency: 4, maxTouchPoints: 0, doNotTrack: null, webdriver: false,
  plugins: [], mimeTypes: [], javaEnabled() { return false; }, sendBeacon() { return true; }, serviceWorker: undefined, clipboard: undefined,
});
pub("performance", { now: (() => { const i = Date.now(); return () => Date.now() - i; })(), timeOrigin: Date.now(), mark() {}, measure() {}, getEntries() { return []; }, getEntriesByType() { return []; }, getEntriesByName() { return []; }, clearMarks() {}, clearMeasures() {}, timing: { navigationStart: Date.now() } });
pub("btoa", s => n.base64(String(s)));
pub("atob", s => n.de64(String(s)));
pub("crypto", { getRandomValues(a) { for (let i = 0; i < a.length; i++) a[i] = Math.floor(Math.random() * 4294967296); return a; }, randomUUID() { const h = () => Math.floor(Math.random() * 65536).toString(16).padStart(4, "0"); return h() + h() + "-" + h() + "-4" + h().slice(1) + "-a" + h().slice(1) + "-" + h() + h() + h(); }, subtle: undefined });
pub("structuredClone", v => {
  const vistos = new Map();
  const c = x => {
    if (typeof x === "function" || typeof x === "symbol") throw new DOMException("could not be cloned", "DataCloneError");
    if (typeof x !== "object" || x === null) return x;
    if (vistos.has(x)) return vistos.get(x);
    if (x instanceof Date) return new Date(x.getTime());
    if (x instanceof RegExp) return new RegExp(x.source, x.flags);
    let r;
    if (Array.isArray(x)) { r = []; vistos.set(x, r); x.forEach((e, i) => { r[i] = c(e); }); return r; }
    if (x instanceof Map) { r = new Map(); vistos.set(x, r); x.forEach((e, k) => r.set(c(k), c(e))); return r; }
    if (x instanceof Set) { r = new Set(); vistos.set(x, r); x.forEach(e => r.add(c(e))); return r; }
    r = {}; vistos.set(x, r); for (const k of Object.keys(x)) r[k] = c(x[k]); return r;
  };
  return c(v);
});

// ------------------------------------------------------------- armazenamento
const dadosLocais = (() => { try { return JSON.parse(n.lerArmazenamento() || "{}"); } catch (e) { return {}; } })();
const criarArmazenamento = (dados, gravar) => {
  const metodos = {
    getItem(k) { k = String(k); return Object.prototype.hasOwnProperty.call(dados, k) ? dados[k] : null; },
    setItem(k, v) { dados[String(k)] = String(v); gravar(); },
    removeItem(k) { delete dados[String(k)]; gravar(); },
    clear() { for (const k of Object.keys(dados)) delete dados[k]; gravar(); },
    key(i) { const k = Object.keys(dados)[i]; return k === undefined ? null : k; },
    get length() { return Object.keys(dados).length; },
  };
  return new Proxy(dados, {
    get(t, k) { if (k in metodos) return metodos[k]; if (k === Symbol.toStringTag) return "Storage"; return metodos.getItem(k) ?? undefined; },
    set(t, k, v) { metodos.setItem(k, v); return true; },
    has(t, k) { return k in metodos || k in t; },
    deleteProperty(t, k) { metodos.removeItem(k); return true; },
    ownKeys(t) { return Object.keys(t); },
  });
};
pub("localStorage", criarArmazenamento(dadosLocais, () => n.gravarArmazenamento(JSON.stringify(dadosLocais))));
pub("sessionStorage", criarArmazenamento({}, () => {}));

// ------------------------------------------------------------- texto e dados
pub("TextEncoder", class TextEncoder { get encoding() { return "utf-8"; } encode(s) { return new Uint8Array(n.codificar(String(s === undefined ? "" : s))); } });
pub("TextDecoder", class TextDecoder { constructor(e) { this.encoding = e || "utf-8"; this.fatal = false; this.ignoreBOM = false; } decode(b) { return b === undefined ? "" : n.decodificar(b); } });
class Blob {
  constructor(p, o) { const u = []; for (const x of p || []) { if (x instanceof Blob) u.push(...x.__b); else if (typeof x === "string") u.push(...new Uint8Array(n.codificar(x))); else if (x instanceof ArrayBuffer) u.push(...new Uint8Array(x)); else if (x && x.buffer) u.push(...new Uint8Array(x.buffer)); else u.push(...new Uint8Array(n.codificar(String(x)))); } oculto(this, "__b", u); this.type = o && o.type ? String(o.type).toLowerCase() : ""; }
  get size() { return this.__b.length; }
  text() { return Promise.resolve(n.decodificar(new Uint8Array(this.__b))); }
  arrayBuffer() { return Promise.resolve(new Uint8Array(this.__b).buffer); }
  slice(a, b, t) { const r = new Blob([], { type: t }); r.__b = this.__b.slice(a, b); return r; }
}
pub("Blob", Blob);
pub("File", class File extends Blob { constructor(p, nome, o) { super(p, o); this.name = nome; this.lastModified = Date.now(); } });
class FormData {
  constructor(f) {
    oculto(this, "__p", []);
    if (f) for (const c of f.elements) {
      if (!c.name || c.disabled) continue;
      const t = c.type;
      if ((t === "checkbox" || t === "radio") && !c.checked) continue;
      if (t === "submit" || t === "button" || t === "reset" || t === "file" || c.tagName === "BUTTON" || c.tagName === "FIELDSET") continue;
      this.__p.push([c.name, c.value]);
    }
  }
  append(a, b) { this.__p.push([String(a), b]); }
  delete(a) { this.__p = this.__p.filter(x => x[0] !== a); }
  get(a) { const x = this.__p.find(y => y[0] === a); return x ? x[1] : null; }
  getAll(a) { return this.__p.filter(y => y[0] === a).map(y => y[1]); }
  has(a) { return this.__p.some(y => y[0] === a); }
  set(a, b) { this.delete(a); this.__p.push([String(a), b]); }
  forEach(f, t) { for (const [a, b] of this.__p) f.call(t, b, a, this); }
  keys() { return this.__p.map(x => x[0])[Symbol.iterator](); }
  values() { return this.__p.map(x => x[1])[Symbol.iterator](); }
  entries() { return this.__p.map(x => [x[0], x[1]])[Symbol.iterator](); }
  [Symbol.iterator]() { return this.entries(); }
}
pub("FormData", FormData);

// ------------------------------------------------------------- rede
class Headers {
  constructor(i) {
    oculto(this, "__m", new Map());
    if (i instanceof Headers) i.forEach((v, k) => this.append(k, v));
    else if (Array.isArray(i)) for (const [a, b] of i) this.append(a, b);
    else if (i) for (const k of Object.keys(i)) this.append(k, i[k]);
  }
  append(a, b) { a = String(a).toLowerCase(); const v = this.__m.get(a); this.__m.set(a, v === undefined ? String(b) : v + ", " + b); }
  set(a, b) { this.__m.set(String(a).toLowerCase(), String(b)); }
  get(a) { const v = this.__m.get(String(a).toLowerCase()); return v === undefined ? null : v; }
  has(a) { return this.__m.has(String(a).toLowerCase()); }
  delete(a) { this.__m.delete(String(a).toLowerCase()); }
  forEach(f, t) { [...this.__m.keys()].sort().forEach(k => f.call(t, this.__m.get(k), k, this)); }
  keys() { return [...this.__m.keys()].sort()[Symbol.iterator](); }
  values() { return [...this.__m.keys()].sort().map(k => this.__m.get(k))[Symbol.iterator](); }
  entries() { return [...this.__m.keys()].sort().map(k => [k, this.__m.get(k)])[Symbol.iterator](); }
  [Symbol.iterator]() { return this.entries(); }
}
pub("Headers", Headers);
const lerCabecalhos = t => { const h = new Headers(); for (const l of t.split("\r\n")) { const i = l.indexOf(":"); if (i > 0) h.append(l.slice(0, i), l.slice(i + 1).trim()); } return h; };
const corpoDe = (b, h) => {
  if (b === undefined || b === null) return null;
  if (typeof b === "string") { if (!h.has("content-type")) h.set("content-type", "text/plain;charset=UTF-8"); return b; }
  if (b instanceof URLSearchParams) { if (!h.has("content-type")) h.set("content-type", "application/x-www-form-urlencoded;charset=UTF-8"); return b.toString(); }
  if (b instanceof FormData) { const s = new URLSearchParams(); b.forEach((v, k) => s.append(k, String(v))); if (!h.has("content-type")) h.set("content-type", "application/x-www-form-urlencoded;charset=UTF-8"); return s.toString(); }
  if (b instanceof Blob) { if (b.type && !h.has("content-type")) h.set("content-type", b.type); return new Uint8Array(b.__b).buffer; }
  if (b instanceof ArrayBuffer) return b;
  if (b.buffer instanceof ArrayBuffer) return b.buffer;
  return String(b);
};
class Response {
  constructor(b, o) { o = o || {}; this.status = o.status === undefined ? 200 : o.status; this.statusText = o.statusText || ""; this.ok = this.status >= 200 && this.status < 300; this.headers = new Headers(o.headers); this.url = o.__url || ""; this.redirected = false; this.type = "basic"; this.bodyUsed = false; oculto(this, "__c", b === undefined ? null : b); }
  __t() { if (this.bodyUsed) return Promise.reject(new TypeError("body stream already read")); this.bodyUsed = true; return Promise.resolve(this.__c); }
  text() { return this.__t().then(b => b === null ? "" : typeof b === "string" ? b : n.decodificar(b)); }
  json() { return this.text().then(JSON.parse); }
  arrayBuffer() { return this.__t().then(b => b === null ? new ArrayBuffer(0) : typeof b === "string" ? n.codificar(b) : b); }
  blob() { return this.arrayBuffer().then(b => new Blob([b], { type: this.headers.get("content-type") || "" })); }
  clone() { return new Response(this.__c, { status: this.status, statusText: this.statusText, headers: this.headers, __url: this.url }); }
  static json(v, o) { const r = new Response(JSON.stringify(v), o); r.headers.set("content-type", "application/json"); return r; }
  static error() { const r = new Response(null, { status: 200 }); r.status = 0; r.ok = false; r.type = "error"; return r; }
}
pub("Response", Response);
class Request {
  constructor(i, o) { o = o || {}; const r = i instanceof Request ? i : null; this.url = r ? r.url : new URL(String(i), n.url()).href; this.method = String(o.method || (r && r.method) || "GET").toUpperCase(); this.headers = new Headers(o.headers || (r && r.headers)); oculto(this, "__c", o.body !== undefined ? o.body : r ? r.__c : null); this.signal = o.signal || (r && r.signal) || null; this.mode = o.mode || "cors"; this.credentials = o.credentials || "same-origin"; }
  clone() { return new Request(this); }
}
pub("Request", Request);
pub("fetch", (i, o) => new Promise((ok, no) => {
  let q;
  try { q = new Request(i, o); } catch (e) { return no(e); }
  if (q.signal && q.signal.aborted) return no(q.signal.reason || new DOMException("Aborted", "AbortError"));
  const b = corpoDe(q.__c, q.headers);
  const lista = []; q.headers.forEach((v, k) => lista.push([k, v]));
  const id = n.pedir(q.method, q.url, lista, b, (status, url, cab, corpo, erro) => {
    if (erro) return no(new TypeError("Failed to fetch: " + erro));
    ok(new Response(q.method === "HEAD" ? null : corpo, { status, headers: lerCabecalhos(cab), __url: url }));
  });
  if (q.signal) q.signal.addEventListener("abort", () => { n.cancelarPedido(id); no(q.signal.reason || new DOMException("Aborted", "AbortError")); });
}));
class AbortSignal extends EventTarget {
  constructor() { return Object.setPrototypeOf(new AlvoSimples(), AbortSignal.prototype); }
}
function AlvoSimples() { oculto(this, "aborted", false); oculto(this, "reason", undefined); oculto(this, "onabort", null); oculto(this, "__o", []); }
AlvoSimples.prototype = Object.create(Object.prototype);
AbortSignal.prototype.addEventListener = function (t, f) { this.__o.push(f); };
AbortSignal.prototype.removeEventListener = function (t, f) { this.__o = this.__o.filter(x => x !== f); };
AbortSignal.prototype.dispatchEvent = function (e) { for (const f of this.__o.slice()) f.call(this, e); if (this.onabort) this.onabort(e); return true; };
AbortSignal.prototype.throwIfAborted = function () { if (this.aborted) throw this.reason; };
AbortSignal.abort = r => { const s = new AbortSignal(); s.aborted = true; s.reason = r === undefined ? new DOMException("Aborted", "AbortError") : r; return s; };
AbortSignal.timeout = ms => { const c = new AbortController(); setTimeout(() => c.abort(new DOMException("Timeout", "TimeoutError")), ms); return c.signal; };
pub("AbortSignal", AbortSignal);
class AbortController { constructor() { this.signal = new AbortSignal(); } abort(r) { const s = this.signal; if (s.aborted) return; s.aborted = true; s.reason = r === undefined ? new DOMException("Aborted", "AbortError") : r; s.dispatchEvent(new Event("abort")); } }
pub("AbortController", AbortController);
class XMLHttpRequest {
  constructor() { this.readyState = 0; this.status = 0; this.statusText = ""; this.responseText = ""; this.response = ""; this.responseType = ""; this.responseURL = ""; this.timeout = 0; this.withCredentials = false; this.upload = {}; oculto(this, "__h", new Headers()); oculto(this, "__o", {}); oculto(this, "__r", null); oculto(this, "__id", 0); for (const k of ["onreadystatechange", "onload", "onerror", "onabort", "onloadend", "onloadstart", "onprogress", "ontimeout"]) this[k] = null; }
  addEventListener(t, f) { (this.__o[t] = this.__o[t] || []).push(f); }
  removeEventListener(t, f) { if (this.__o[t]) this.__o[t] = this.__o[t].filter(x => x !== f); }
  dispatchEvent(e) { e.target = this; for (const f of (this.__o[e.type] || []).slice()) f.call(this, e); const h = this["on" + e.type]; if (typeof h === "function") h.call(this, e); return true; }
  __e(t) { this.dispatchEvent(new ProgressEvent(t)); }
  open(m, u, a) { this.__m = String(m); this.__u = new URL(String(u), n.url()).href; this.readyState = 1; this.__h = new Headers(); this.__r = null; this.status = 0; this.responseText = ""; this.response = ""; this.__e("readystatechange"); }
  setRequestHeader(a, b) { this.__h.append(a, b); }
  overrideMimeType() {}
  getResponseHeader(a) { return this.__r ? this.__r.get(a) : null; }
  getAllResponseHeaders() { if (!this.__r) return ""; let s = ""; this.__r.forEach((v, k) => { s += k + ": " + v + "\r\n"; }); return s; }
  abort() { if (this.__id) n.cancelarPedido(this.__id); this.__id = 0; this.readyState = 0; this.__e("abort"); this.__e("loadend"); }
  send(b) {
    const corpo = corpoDe(b, this.__h);
    const lista = []; this.__h.forEach((v, k) => lista.push([k, v]));
    this.__e("loadstart");
    this.__id = n.pedir(this.__m, this.__u, lista, corpo, (status, url, cab, dados, erro) => {
      this.__id = 0;
      if (erro) { this.readyState = 4; this.__e("readystatechange"); this.__e("error"); this.__e("loadend"); return; }
      this.status = status; this.responseURL = url; this.__r = lerCabecalhos(cab);
      this.readyState = 2; this.__e("readystatechange");
      this.readyState = 3; this.__e("readystatechange");
      const t = this.responseType;
      if (t === "arraybuffer") this.response = dados;
      else if (t === "blob") this.response = new Blob([dados], { type: this.__r.get("content-type") || "" });
      else {
        this.responseText = n.decodificar(dados);
        if (t === "json") { try { this.response = JSON.parse(this.responseText); } catch (e) { this.response = null; } }
        else this.response = this.responseText;
      }
      this.readyState = 4; this.__e("readystatechange"); this.__e("load"); this.__e("loadend");
    });
  }
}
XMLHttpRequest.UNSENT = 0; XMLHttpRequest.OPENED = 1; XMLHttpRequest.HEADERS_RECEIVED = 2; XMLHttpRequest.LOADING = 3; XMLHttpRequest.DONE = 4;
pub("XMLHttpRequest", XMLHttpRequest);

// ------------------------------------------------------------- observadores
pub("MutationObserver", class MutationObserver { constructor(f) { this.f = f; } observe() {} disconnect() {} takeRecords() { return []; } });
pub("ResizeObserver", class ResizeObserver { constructor(f) { this.f = f; } observe(el) { const r = el.getBoundingClientRect(); setTimeout(() => this.f([{ target: el, contentRect: r }], this), 0); } unobserve() {} disconnect() {} });
pub("IntersectionObserver", class IntersectionObserver { constructor(f, o) { this.f = f; this.root = null; this.rootMargin = "0px"; this.thresholds = [0]; } observe(el) { setTimeout(() => { const r = el.getBoundingClientRect(); this.f([{ target: el, isIntersecting: true, intersectionRatio: 1, boundingClientRect: r, intersectionRect: r, rootBounds: null, time: Date.now() }], this); }, 0); } unobserve() {} disconnect() {} takeRecords() { return []; } });
pub("PerformanceObserver", class PerformanceObserver { observe() {} disconnect() {} takeRecords() { return []; } });
pub("requestIdleCallback", f => setTimeout(() => f({ didTimeout: false, timeRemaining: () => 50 }), 1));
pub("cancelIdleCallback", i => clearTimeout(i));
pub("customElements", { define() {}, get() { return undefined; }, whenDefined() { return new Promise(() => {}); }, upgrade() {} });
pub("CSS", { supports(a, b) { return b !== undefined || /:/.test(String(a)); }, escape: s => String(s).replace(/[^\w-]/g, c => "\\" + c) });
pub("Image", function Image(w, h) { const e = document.createElement("img"); if (w !== undefined) e.width = w; if (h !== undefined) e.height = h; return e; });
pub("Option", function Option(t, v, d, s) { const e = document.createElement("option"); if (t !== undefined) e.text = t; if (v !== undefined) e.value = v; if (s) e.selected = true; return e; });
// Outros documentos: nós soltos na mesma árvore (bastam para ler e montar HTML).
const novoDocumento = (t) => {
  const html = document.createElement("html"), head = document.createElement("head"), body = document.createElement("body");
  html.append(head, body);
  if (t !== undefined) { const ti = document.createElement("title"); ti.textContent = t; head.appendChild(ti); }
  const d = {
    nodeType: 9, nodeName: "#document", documentElement: html, head, body, defaultView: null, readyState: "complete",
    createElement: x => document.createElement(x), createElementNS: (a, x) => document.createElement(x), createTextNode: x => document.createTextNode(x),
    createComment: x => document.createComment(x), createDocumentFragment: () => document.createDocumentFragment(), importNode: (a, b) => a.cloneNode(b), adoptNode: a => a,
    querySelector: s => html.querySelector(s), querySelectorAll: s => html.querySelectorAll(s), getElementsByTagName: s => html.getElementsByTagName(s), getElementsByClassName: s => html.getElementsByClassName(s),
    getElementById: i => html.querySelector("#" + CSS.escape(i)), addEventListener() {}, removeEventListener() {},
    get title() { const x = head.querySelector("title"); return x ? x.textContent : ""; }, set title(v) { let x = head.querySelector("title"); if (!x) { x = document.createElement("title"); head.appendChild(x); } x.textContent = v; },
    get forms() { return html.querySelectorAll("form"); }, get images() { return html.querySelectorAll("img"); }, get links() { return html.querySelectorAll("a[href]"); },
  };
  return d;
};
Object.defineProperty(document, "implementation", { configurable: true, value: { createHTMLDocument: novoDocumento, hasFeature: () => true, createDocumentType: () => ({ nodeType: 10 }), createDocument: () => novoDocumento() } });
pub("DOMParser", class DOMParser {
  parseFromString(t) {
    const d = novoDocumento();
    const m = /<body[^>]*>([\s\S]*)<\/body>/i.exec(t);
    const c = /<head[^>]*>([\s\S]*)<\/head>/i.exec(t);
    if (c) d.head.innerHTML = c[1];
    d.body.innerHTML = m ? m[1] : t;
    return d;
  }
});
pub("XMLSerializer", class XMLSerializer { serializeToString(n) { return n.outerHTML !== undefined ? n.outerHTML : (n.documentElement ? n.documentElement.outerHTML : String(n.textContent)); } });
pub("Range", class Range { constructor() { this.collapsed = true; } setStart() {} setEnd() {} selectNode() {} selectNodeContents() {} collapse() {} cloneRange() { return new Range(); } detach() {} getBoundingClientRect() { return { x: 0, y: 0, left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 }; } getClientRects() { return []; } deleteContents() {} createContextualFragment(h) { const t = document.createElement("div"); t.innerHTML = h; const f = document.createDocumentFragment(); while (t.firstChild) f.appendChild(t.firstChild); return f; } });
document.createRange = () => new Range();

// ------------------------------------------------------------- Intl (o básico, em português)
const milhares = (x, min, max) => {
  const neg = x < 0; x = Math.abs(x);
  let t = x.toFixed(max === undefined ? Math.min(3, Math.max(min || 0, (String(x).split(".")[1] || "").length)) : max);
  let [i, f] = t.split(".");
  i = i.replace(/\B(?=(\d{3})+(?!\d))/g, ".");
  if (f !== undefined && max !== undefined) { while (f.length > (min || 0) && f.endsWith("0")) f = f.slice(0, -1); }
  return (neg ? "-" : "") + i + (f ? "," + f : "");
};
const Intl = {
  NumberFormat: class NumberFormat { constructor(l, o) { this.o = o || {}; } format(x) { const o = this.o; if (o.style === "percent") return milhares(x * 100, 0, o.maximumFractionDigits || 0) + "%"; const t = milhares(+x, o.minimumFractionDigits, o.maximumFractionDigits); return o.style === "currency" ? (o.currency === "BRL" ? "R$ " : o.currency + " ") + milhares(+x, 2, 2) : t; } resolvedOptions() { return { locale: "pt-BR", ...this.o }; } formatToParts(x) { return [{ type: "integer", value: this.format(x) }]; } },
  DateTimeFormat: class DateTimeFormat { constructor(l, o) { this.o = o || {}; } format(d) { d = d === undefined ? new Date() : new Date(d); return this.o.timeStyle || this.o.hour ? d.toLocaleString() : d.toLocaleDateString(); } resolvedOptions() { return { locale: "pt-BR", timeZone: "UTC", calendar: "gregory", numberingSystem: "latn" }; } formatToParts(d) { return [{ type: "literal", value: this.format(d) }]; } },
  Collator: class Collator { constructor() {} get compare() { return (a, b) => String(a).localeCompare(String(b)); } resolvedOptions() { return { locale: "pt-BR" }; } },
  PluralRules: class PluralRules { select(x) { return x === 1 ? "one" : "other"; } },
  RelativeTimeFormat: class RelativeTimeFormat { format(v, u) { return v + " " + u; } },
  ListFormat: class ListFormat { format(l) { l = [...l]; return l.length < 2 ? l.join("") : l.slice(0, -1).join(", ") + " e " + l[l.length - 1]; } },
  getCanonicalLocales: l => [].concat(l || []),
};
pub("Intl", Intl);
})
