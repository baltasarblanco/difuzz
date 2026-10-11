if (false) { try {} catch (e) {} }
let s = "";
const f = () => { if (String(x) === "zz") s = "q"; };
const h = () => { s = "abc".substring(1, undefined); };
(() => {}).constructor("f", "h", atob("dHJ5IHsgZigpOyB9IGNhdGNoIChlKSB7IGgoKTsgfQ=="))(f, h);
console.log(JSON.stringify(s));
let x = 1;
