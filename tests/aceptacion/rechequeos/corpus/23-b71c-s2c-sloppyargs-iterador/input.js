function al(a) { arguments[0] = 2; return a; }
let s = [].join();
if (al(1) === 2) s = "abc".substring(1, undefined);
console.log(JSON.stringify(s));
Object.getPrototypeOf([][Symbol.iterator]()).next = function () { return { done: true, value: undefined }; };
