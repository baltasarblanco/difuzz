let result = "none";
class K { toString() { result = "abc".substring(1, undefined); return "k"; } }
console.log(`${new K()}`);
console.log(result);
