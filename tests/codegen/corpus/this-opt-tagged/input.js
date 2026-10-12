var o = { f(){ return this === o } }; console.log((o?.f)`x`, (o.f)`x`, (0, o.f)`x`);
