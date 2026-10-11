let d = false;
if (typeof window === "undefined") d = delete (0 / 0);
console.log(d);
