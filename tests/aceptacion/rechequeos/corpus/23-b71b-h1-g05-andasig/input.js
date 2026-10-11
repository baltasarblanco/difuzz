let s = ""; let u = typeof window === "undefined"; u &&= (s = "abc".substring(1, undefined)); console.log(JSON.stringify(s));
