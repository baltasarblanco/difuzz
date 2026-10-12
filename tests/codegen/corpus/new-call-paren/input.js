function f(){ return function C(){ this.k = "inner" } } f.prototype.k = "outer"; var r = new (f())(); console.log(r.k); var s = new f()(); console.log(typeof s);
