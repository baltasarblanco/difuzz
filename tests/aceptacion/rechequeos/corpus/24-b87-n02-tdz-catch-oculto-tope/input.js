if (false) { try {} catch (e) {} }
let s = "";
const f = () => { if (String(x) === "zz") s = "q"; };
const h = () => { s = "abc".substring(1, undefined); };
(() => {}).constructor("f", "h", atob("Zm9yIChsZXQgaSA9IDA7IGkgPCAzMDA7IGkrKykgeyB0cnkgeyBmKCk7IH0gY2F0Y2ggKGUpIHt9IH0gdHJ5IHsgZigpOyB9IGNhdGNoIChlKSB7IGgoKTsgfQ=="))(f, h);
console.log(JSON.stringify(s));
let x = 1;
