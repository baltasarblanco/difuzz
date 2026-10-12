class A { ["constructor"](){ return "m" } } console.log(A.prototype.hasOwnProperty("constructor"), A.prototype.constructor === A, new A() instanceof A);
