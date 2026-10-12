function g(f = (0, () => {})) { return f.name } console.log(JSON.stringify(g()));
