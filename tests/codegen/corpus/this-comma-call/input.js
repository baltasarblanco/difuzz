var o = { f(){ return this === o } }; console.log((0, o.f)(), (o.f)(), (o?.f)(), o.f());
