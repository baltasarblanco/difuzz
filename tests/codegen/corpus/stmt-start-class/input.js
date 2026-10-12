(class A { static { console.log("static block", this.name) } }); console.log((class B {}).name);
