let x = 1; export default (x); x = 2; import(import.meta.url).then((m) => console.log(m.default));
