// A hand-written stand-in for YouTube's web player script (base.js), shaped like the real one:
// the whole player inside one function, a helper object whose methods scramble a signature,
// a signature function that calls them, an n-parameter function stored in an array and
// guarded by a typeof check, a global string table, and the signature timestamp. It is not
// copied from YouTube; it only has the same structure, so the extractor's searches can be
// tested.
var _yt_player = {};
(function(g) {
var window = this;
var Tz = "split;join;reverse;unshift;forEach".split(";");
var fK = { label: "player", re: /[{}]/g, nested: { depth: 2 } };
var Xy = {
  Kp: function(a, b) { var c = a[0]; a[0] = a[b % a.length]; a[b % a.length] = c; },
  Q6: function(a) { a.reverse(); },
  yN: function(a, b) { a.splice(0, b); }
};
Hza=function(a){a=a.split("");Xy.Kp(a,3);Xy.Q6(a,52);Xy.yN(a,2);Xy.Kp(a,19);return a.join("")};
var Bua=[Lka];
Lka=function(a){var b=a[Tz[0]](""),c=[function(d,e){e=(e%d.length+d.length)%d.length;d.splice(-e).reverse()[Tz[4]](function(f){d[Tz[3]](f)})},"{}",-8,function(d){d[Tz[2]]()},b,/[}{]/];if(typeof Qwe==="undefined")return a;c[0](c[4],c[2]);c[3](c[4]);return b[Tz[1]]("")};
var config = { signatureTimestamp: 20160, sts: 20160 };
g.decode = function(c, b) {
  c.s && (c.s = Hza(decodeURIComponent(c.s)));
  (b = c.get("n")) && (b = Bua[0](b), c.set("n", b));
};
})(_yt_player);
