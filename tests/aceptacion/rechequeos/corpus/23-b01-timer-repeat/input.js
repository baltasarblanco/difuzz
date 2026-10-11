var t = Number.isInteger(5e-324) ? setInterval(function () {}, 50) : setTimeout(function () {}, 50);
clearTimeout(t);
