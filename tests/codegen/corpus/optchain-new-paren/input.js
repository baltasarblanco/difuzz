var a = { b: function(){ this.v = 1 } }; console.log(new (a?.b)().v);
