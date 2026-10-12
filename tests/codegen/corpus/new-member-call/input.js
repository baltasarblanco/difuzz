var a = { b(){ return function D(){ this.z = 2 } } }; console.log(new (a.b())().z);
