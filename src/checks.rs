//! Chequeos estaticos sobre el AST, sin ejecutar nada.

use std::collections::HashSet;

use swc_ecma_ast::{
    AssignExpr, AssignTarget, BindingIdent, CatchClause, Class, ClassDecl, ClassExpr, Expr, FnDecl,
    FnExpr, Ident, ImportDefaultSpecifier, ImportNamedSpecifier, ImportStarAsSpecifier, Lit,
    MemberExpr, MemberProp, Param, Pat, Program, Prop, PropName, PropOrSpread, SimpleAssignTarget,
    UpdateExpr, VarDeclarator, WithStmt,
};
use swc_ecma_visit::{Visit, VisitWith};

/// Asunciones de AGENTS.md que se ven en el fuente y no en runtime:
/// `arguments.callee`, `.caller`, bindings llamados `undefined`, `NaN` o
/// `Infinity`, y helpers de coercion propios (`valueOf`, `toString`,
/// `Symbol.toPrimitive`), que AGENTS.md asume sin efectos. Se suman a los flags
/// del preload (known.rs).
pub fn static_flags(program: &Program) -> Vec<&'static str> {
    let mut v = StaticFlags::default();
    program.visit_with(&mut v);
    let mut out = Vec::new();
    if v.callee {
        out.push("callee");
    }
    if v.redefines_global {
        out.push("redefine_global");
    }
    if v.coercion {
        out.push("coercion");
    }
    // M13: asunciones de oxc (ASSUMPTIONS.md) que se ven en el fuente. Solo las
    // clasifica la tabla de oxc (known.rs::FLAG_REASONS_OXC).
    if v.with {
        out.push("with");
    }
    if v.refs.iter().any(|r| !v.declared.contains(r) && NODE_MISSING_OXC_GLOBALS.binary_search(&r.as_str()).is_ok()) {
        out.push("global_conocido");
    }
    if v.extends_literal {
        out.push("extends");
    }
    out
}

#[derive(Default)]
struct StaticFlags {
    callee: bool,
    redefines_global: bool,
    coercion: bool,
    /// M13: hay un `with` (oxc: "No `with` Statement").
    with: bool,
    /// M13: `class extends <literal, regex, array, objeto o template>`: lanza al
    /// definirse (oxc: "No side effects from extending a class").
    extends_literal: bool,
    /// M13: nombres declarados en cualquier parte del programa y nombres leidos
    /// como expresion. Una lectura libre de un global conocido de oxc que no
    /// existe en node es `global_conocido`. Aproximado: un nombre declarado en
    /// otro scope cuenta como declarado (falso negativo del flag, no positivo).
    declared: HashSet<String>,
    refs: HashSet<String>,
}

/// M13: globals de `KNOWN_GLOBAL_IDENTIFIERS` de oxc (oxc_ecmascript 0.153.0,
/// src/side_effects/known_globals.rs:276-403, privada) que NO existen en node
/// (`!(k in globalThis)` con node v24.14.1, 544 de 639). oxc asume que existen y
/// que leerlos no tiene efectos; en node leerlos lanza ReferenceError. Ordenada
/// (byte order) para binary_search. Se regenera si cambia la version de oxc.
#[rustfmt::skip]
const NODE_MISSING_OXC_GLOBALS: &[&str] = &[
    "AnalyserNode", "Animation", "AnimationEffect", "AnimationEvent", "AnimationPlaybackEvent",
    "AnimationTimeline", "Attr", "Audio", "AudioBuffer", "AudioBufferSourceNode",
    "AudioDestinationNode", "AudioListener", "AudioNode", "AudioParam", "AudioProcessingEvent",
    "AudioScheduledSourceNode", "BarProp", "BeforeUnloadEvent", "BiquadFilterNode", "BlobEvent",
    "CDATASection", "CSS", "CSSAnimation", "CSSFontFaceRule", "CSSImportRule", "CSSKeyframeRule",
    "CSSKeyframesRule", "CSSMediaRule", "CSSNamespaceRule", "CSSPageRule", "CSSRule", "CSSRuleList",
    "CSSStyleDeclaration", "CSSStyleRule", "CSSStyleSheet", "CSSSupportsRule", "CSSTransition",
    "CanvasGradient", "CanvasPattern", "CanvasRenderingContext2D", "ChannelMergerNode",
    "ChannelSplitterNode", "CharacterData", "ClipboardEvent", "Comment", "CompositionEvent",
    "ConvolverNode", "CustomElementRegistry", "DOMImplementation", "DOMMatrix", "DOMMatrixReadOnly",
    "DOMParser", "DOMPoint", "DOMPointReadOnly", "DOMQuad", "DOMRect", "DOMRectList",
    "DOMRectReadOnly", "DOMStringList", "DOMStringMap", "DOMTokenList", "DataTransfer",
    "DataTransferItem", "DataTransferItemList", "DelayNode", "Document", "DocumentFragment",
    "DocumentTimeline", "DocumentType", "DragEvent", "DynamicsCompressorNode", "Element",
    "ErrorEvent", "EventSource", "FileList", "FileReader", "FocusEvent", "FontFace", "GainNode",
    "Gamepad", "GamepadButton", "GamepadEvent", "Geolocation", "GeolocationPositionError",
    "HTMLAllCollection", "HTMLAnchorElement", "HTMLAreaElement", "HTMLAudioElement",
    "HTMLBRElement", "HTMLBaseElement", "HTMLBodyElement", "HTMLButtonElement", "HTMLCanvasElement",
    "HTMLCollection", "HTMLDListElement", "HTMLDataElement", "HTMLDataListElement",
    "HTMLDetailsElement", "HTMLDirectoryElement", "HTMLDivElement", "HTMLDocument", "HTMLElement",
    "HTMLEmbedElement", "HTMLFieldSetElement", "HTMLFontElement", "HTMLFormControlsCollection",
    "HTMLFormElement", "HTMLFrameElement", "HTMLFrameSetElement", "HTMLHRElement",
    "HTMLHeadElement", "HTMLHeadingElement", "HTMLHtmlElement", "HTMLIFrameElement",
    "HTMLImageElement", "HTMLInputElement", "HTMLLIElement", "HTMLLabelElement",
    "HTMLLegendElement", "HTMLLinkElement", "HTMLMapElement", "HTMLMarqueeElement",
    "HTMLMediaElement", "HTMLMenuElement", "HTMLMetaElement", "HTMLMeterElement", "HTMLModElement",
    "HTMLOListElement", "HTMLObjectElement", "HTMLOptGroupElement", "HTMLOptionElement",
    "HTMLOptionsCollection", "HTMLOutputElement", "HTMLParagraphElement", "HTMLParamElement",
    "HTMLPictureElement", "HTMLPreElement", "HTMLProgressElement", "HTMLQuoteElement",
    "HTMLScriptElement", "HTMLSelectElement", "HTMLSlotElement", "HTMLSourceElement",
    "HTMLSpanElement", "HTMLStyleElement", "HTMLTableCaptionElement", "HTMLTableCellElement",
    "HTMLTableColElement", "HTMLTableElement", "HTMLTableRowElement", "HTMLTableSectionElement",
    "HTMLTemplateElement", "HTMLTextAreaElement", "HTMLTimeElement", "HTMLTitleElement",
    "HTMLTrackElement", "HTMLUListElement", "HTMLUnknownElement", "HTMLVideoElement",
    "HashChangeEvent", "History", "IDBCursor", "IDBCursorWithValue", "IDBDatabase", "IDBFactory",
    "IDBIndex", "IDBKeyRange", "IDBObjectStore", "IDBOpenDBRequest", "IDBRequest", "IDBTransaction",
    "IDBVersionChangeEvent", "Image", "ImageData", "InputEvent", "IntersectionObserver",
    "IntersectionObserverEntry", "KeyboardEvent", "KeyframeEffect", "Location", "MediaCapabilities",
    "MediaElementAudioSourceNode", "MediaEncryptedEvent", "MediaError", "MediaList",
    "MediaQueryList", "MediaQueryListEvent", "MediaRecorder", "MediaSource", "MediaStream",
    "MediaStreamAudioDestinationNode", "MediaStreamAudioSourceNode", "MediaStreamTrack",
    "MediaStreamTrackEvent", "MimeType", "MimeTypeArray", "MouseEvent", "MutationEvent",
    "MutationObserver", "MutationRecord", "NamedNodeMap", "Node", "NodeFilter", "NodeIterator",
    "NodeList", "Notification", "OfflineAudioCompletionEvent", "Option", "OscillatorNode",
    "PageTransitionEvent", "Path2D", "PerformanceNavigation", "PerformanceTiming", "PeriodicWave",
    "Plugin", "PluginArray", "PointerEvent", "PopStateEvent", "ProcessingInstruction",
    "ProgressEvent", "PromiseRejectionEvent", "RTCCertificate", "RTCDTMFSender",
    "RTCDTMFToneChangeEvent", "RTCDataChannel", "RTCDataChannelEvent", "RTCIceCandidate",
    "RTCPeerConnection", "RTCPeerConnectionIceEvent", "RTCRtpReceiver", "RTCRtpSender",
    "RTCRtpTransceiver", "RTCSessionDescription", "RTCStatsReport", "RTCTrackEvent",
    "RadioNodeList", "Range", "ResizeObserver", "ResizeObserverEntry", "SVGAElement", "SVGAngle",
    "SVGAnimateElement", "SVGAnimateMotionElement", "SVGAnimateTransformElement",
    "SVGAnimatedAngle", "SVGAnimatedBoolean", "SVGAnimatedEnumeration", "SVGAnimatedInteger",
    "SVGAnimatedLength", "SVGAnimatedLengthList", "SVGAnimatedNumber", "SVGAnimatedNumberList",
    "SVGAnimatedPreserveAspectRatio", "SVGAnimatedRect", "SVGAnimatedString",
    "SVGAnimatedTransformList", "SVGAnimationElement", "SVGCircleElement", "SVGClipPathElement",
    "SVGComponentTransferFunctionElement", "SVGDefsElement", "SVGDescElement", "SVGElement",
    "SVGEllipseElement", "SVGFEBlendElement", "SVGFEColorMatrixElement",
    "SVGFEComponentTransferElement", "SVGFECompositeElement", "SVGFEConvolveMatrixElement",
    "SVGFEDiffuseLightingElement", "SVGFEDisplacementMapElement", "SVGFEDistantLightElement",
    "SVGFEDropShadowElement", "SVGFEFloodElement", "SVGFEFuncAElement", "SVGFEFuncBElement",
    "SVGFEFuncGElement", "SVGFEFuncRElement", "SVGFEGaussianBlurElement", "SVGFEImageElement",
    "SVGFEMergeElement", "SVGFEMergeNodeElement", "SVGFEMorphologyElement", "SVGFEOffsetElement",
    "SVGFEPointLightElement", "SVGFESpecularLightingElement", "SVGFESpotLightElement",
    "SVGFETileElement", "SVGFETurbulenceElement", "SVGFilterElement", "SVGForeignObjectElement",
    "SVGGElement", "SVGGeometryElement", "SVGGradientElement", "SVGGraphicsElement",
    "SVGImageElement", "SVGLength", "SVGLengthList", "SVGLineElement", "SVGLinearGradientElement",
    "SVGMPathElement", "SVGMarkerElement", "SVGMaskElement", "SVGMatrix", "SVGMetadataElement",
    "SVGNumber", "SVGNumberList", "SVGPathElement", "SVGPatternElement", "SVGPoint", "SVGPointList",
    "SVGPolygonElement", "SVGPolylineElement", "SVGPreserveAspectRatio", "SVGRadialGradientElement",
    "SVGRect", "SVGRectElement", "SVGSVGElement", "SVGScriptElement", "SVGSetElement",
    "SVGStopElement", "SVGStringList", "SVGStyleElement", "SVGSwitchElement", "SVGSymbolElement",
    "SVGTSpanElement", "SVGTextContentElement", "SVGTextElement", "SVGTextPathElement",
    "SVGTextPositioningElement", "SVGTitleElement", "SVGTransform", "SVGTransformList",
    "SVGUnitTypes", "SVGUseElement", "SVGViewElement", "Screen", "ScriptProcessorNode",
    "SecurityPolicyViolationEvent", "Selection", "ShadowRoot", "SourceBuffer", "SourceBufferList",
    "SpeechSynthesisEvent", "SpeechSynthesisUtterance", "StaticRange", "Storage", "StorageEvent",
    "StyleSheet", "StyleSheetList", "Text", "TextMetrics", "TextTrack", "TextTrackCue",
    "TextTrackCueList", "TextTrackList", "TimeRanges", "TrackEvent", "TransitionEvent",
    "TreeWalker", "UIEvent", "VTTCue", "ValidityState", "VisualViewport", "WaveShaperNode",
    "WebGLActiveInfo", "WebGLBuffer", "WebGLContextEvent", "WebGLFramebuffer", "WebGLProgram",
    "WebGLQuery", "WebGLRenderbuffer", "WebGLRenderingContext", "WebGLSampler", "WebGLShader",
    "WebGLShaderPrecisionFormat", "WebGLSync", "WebGLTexture", "WebGLUniformLocation",
    "WebKitCSSMatrix", "WheelEvent", "Window", "Worker", "XMLDocument", "XMLHttpRequest",
    "XMLHttpRequestEventTarget", "XMLHttpRequestUpload", "XMLSerializer", "XPathEvaluator",
    "XPathExpression", "XPathResult", "XSLTProcessor", "alert", "blur", "cancelAnimationFrame",
    "captureEvents", "close", "closed", "confirm", "customElements", "devicePixelRatio", "document",
    "event", "find", "focus", "frameElement", "frames", "getComputedStyle", "getSelection",
    "history", "indexedDB", "isSecureContext", "length", "location", "locationbar", "matchMedia",
    "menubar", "moveBy", "moveTo", "name", "onabort", "onafterprint", "onanimationend",
    "onanimationiteration", "onanimationstart", "onbeforeprint", "onbeforeunload", "onblur",
    "oncanplay", "oncanplaythrough", "onchange", "onclick", "oncontextmenu", "oncuechange",
    "ondblclick", "ondrag", "ondragend", "ondragenter", "ondragleave", "ondragover", "ondragstart",
    "ondrop", "ondurationchange", "onemptied", "onended", "onerror", "onfocus",
    "ongotpointercapture", "onhashchange", "oninput", "oninvalid", "onkeydown", "onkeypress",
    "onkeyup", "onlanguagechange", "onload", "onloadeddata", "onloadedmetadata", "onloadstart",
    "onlostpointercapture", "onmessage", "onmousedown", "onmouseenter", "onmouseleave",
    "onmousemove", "onmouseout", "onmouseover", "onmouseup", "onoffline", "ononline", "onpagehide",
    "onpageshow", "onpause", "onplay", "onplaying", "onpointercancel", "onpointerdown",
    "onpointerenter", "onpointerleave", "onpointermove", "onpointerout", "onpointerover",
    "onpointerup", "onpopstate", "onprogress", "onratechange", "onrejectionhandled", "onreset",
    "onresize", "onscroll", "onseeked", "onseeking", "onselect", "onstalled", "onstorage",
    "onsubmit", "onsuspend", "ontimeupdate", "ontoggle", "ontransitioncancel", "ontransitionend",
    "ontransitionrun", "ontransitionstart", "onunhandledrejection", "onunload", "onvolumechange",
    "onwaiting", "onwebkitanimationend", "onwebkitanimationiteration", "onwebkitanimationstart",
    "onwebkittransitionend", "onwheel", "open", "opener", "origin", "outerHeight", "outerWidth",
    "parent", "personalbar", "postMessage", "print", "prompt", "releaseEvents",
    "requestAnimationFrame", "resizeBy", "resizeTo", "screen", "screenLeft", "screenTop", "screenX",
    "screenY", "scroll", "scrollBy", "scrollTo", "scrollbars", "self", "speechSynthesis", "status",
    "statusbar", "stop", "toolbar", "top", "webkitURL", "window",
];

/// Nombres ligados por un patron de declaracion (var, parametro, catch).
fn pat_names(p: &Pat, out: &mut HashSet<String>) {
    struct Names<'a>(&'a mut HashSet<String>);
    impl Visit for Names<'_> {
        fn visit_binding_ident(&mut self, n: &BindingIdent) {
            self.0.insert(n.id.sym.to_string());
        }
        // Los valores por defecto no declaran nada.
        fn visit_expr(&mut self, _: &Expr) {}
    }
    p.visit_with(&mut Names(out));
}

impl StaticFlags {
    fn check_name(&mut self, id: &Ident) {
        if matches!(id.sym.as_str(), "undefined" | "NaN" | "Infinity") {
            self.redefines_global = true;
        }
    }
}

fn is_coercion_name(name: &str) -> bool {
    matches!(name, "valueOf" | "toString")
}

fn is_symbol_to_primitive(e: &Expr) -> bool {
    matches!(e, Expr::Member(m)
        if matches!(&*m.obj, Expr::Ident(i) if i.sym.as_str() == "Symbol")
            && m.prop.is_ident_with("toPrimitive"))
}

impl Visit for StaticFlags {
    fn visit_with_stmt(&mut self, n: &WithStmt) {
        self.with = true;
        n.visit_children_with(self);
    }

    fn visit_class(&mut self, n: &Class) {
        if let Some(sup) = &n.super_class {
            let mut e = &**sup;
            while let Expr::Paren(p) = e {
                e = &p.expr;
            }
            let lit = match e {
                Expr::Lit(Lit::Null(_)) => false,
                Expr::Lit(_) | Expr::Array(_) | Expr::Object(_) | Expr::Tpl(_) => true,
                _ => false,
            };
            if lit {
                self.extends_literal = true;
            }
        }
        n.visit_children_with(self);
    }

    fn visit_expr(&mut self, n: &Expr) {
        if let Expr::Ident(i) = n {
            self.refs.insert(i.sym.to_string());
        }
        n.visit_children_with(self);
    }

    fn visit_var_declarator(&mut self, n: &VarDeclarator) {
        pat_names(&n.name, &mut self.declared);
        n.visit_children_with(self);
    }

    fn visit_param(&mut self, n: &Param) {
        pat_names(&n.pat, &mut self.declared);
        n.visit_children_with(self);
    }

    fn visit_pat(&mut self, n: &Pat) {
        n.visit_children_with(self);
    }

    fn visit_catch_clause(&mut self, n: &CatchClause) {
        if let Some(p) = &n.param {
            pat_names(p, &mut self.declared);
        }
        n.visit_children_with(self);
    }

    fn visit_class_decl(&mut self, n: &ClassDecl) {
        self.declared.insert(n.ident.sym.to_string());
        n.visit_children_with(self);
    }

    fn visit_class_expr(&mut self, n: &ClassExpr) {
        if let Some(i) = &n.ident {
            self.declared.insert(i.sym.to_string());
        }
        n.visit_children_with(self);
    }

    fn visit_fn_expr(&mut self, n: &FnExpr) {
        if let Some(i) = &n.ident {
            self.declared.insert(i.sym.to_string());
        }
        n.visit_children_with(self);
    }

    fn visit_import_default_specifier(&mut self, n: &ImportDefaultSpecifier) {
        self.declared.insert(n.local.sym.to_string());
    }

    fn visit_import_named_specifier(&mut self, n: &ImportNamedSpecifier) {
        self.declared.insert(n.local.sym.to_string());
    }

    fn visit_import_star_as_specifier(&mut self, n: &ImportStarAsSpecifier) {
        self.declared.insert(n.local.sym.to_string());
    }

    // Claves de objetos literales y de miembros de clase.
    fn visit_prop_name(&mut self, n: &PropName) {
        match n {
            PropName::Ident(i) if is_coercion_name(i.sym.as_str()) => self.coercion = true,
            PropName::Computed(c) if is_symbol_to_primitive(&c.expr) => self.coercion = true,
            _ => {}
        }
        n.visit_children_with(self);
    }

    // `o.valueOf = …`, `C.prototype.toString = …`, `o[Symbol.toPrimitive] = …`.
    fn visit_assign_expr(&mut self, n: &AssignExpr) {
        if let AssignTarget::Simple(SimpleAssignTarget::Member(m)) = &n.left {
            let hit = match &m.prop {
                MemberProp::Ident(i) => is_coercion_name(i.sym.as_str()),
                MemberProp::Computed(c) => is_symbol_to_primitive(&c.expr),
                _ => false,
            };
            if hit {
                self.coercion = true;
            }
        }
        n.visit_children_with(self);
    }

    fn visit_member_expr(&mut self, n: &MemberExpr) {
        let on_arguments = matches!(&*n.obj, Expr::Ident(i) if i.sym.as_str() == "arguments");
        if n.prop.is_ident_with("caller") || (on_arguments && n.prop.is_ident_with("callee")) {
            self.callee = true;
        }
        n.visit_children_with(self);
    }

    // Declaraciones, parametros, catch y destinos de asignacion simples.
    fn visit_binding_ident(&mut self, n: &BindingIdent) {
        self.check_name(&n.id);
        n.visit_children_with(self);
    }

    fn visit_fn_decl(&mut self, n: &FnDecl) {
        self.check_name(&n.ident);
        self.declared.insert(n.ident.sym.to_string());
        n.visit_children_with(self);
    }
}

/// Escrituras sobre un objeto recien creado (`[a, b][0] = x`, `({}).k = x`,
/// `[[a][0]] = …`): nadie puede leerlas. Es la firma del bug de swc#12397 en el
/// fixture 9739, que el observador no ve porque agrega un uso y apaga el
/// disparador. main compara la cuenta del original contra la del minificado:
/// si sube, es sospechoso (no veredicto: si nadie lee el destino, puede ser
/// equivalente).
pub fn dead_writes(program: &Program) -> usize {
    let mut v = DeadWrites(0);
    program.visit_with(&mut v);
    v.0
}

struct DeadWrites(usize);

impl Visit for DeadWrites {
    fn visit_assign_expr(&mut self, n: &AssignExpr) {
        if let AssignTarget::Simple(t) = &n.left {
            let hit = match t {
                SimpleAssignTarget::Member(m) => is_fresh(&m.obj),
                SimpleAssignTarget::Paren(p) => is_write_to_fresh(&p.expr),
                _ => false,
            };
            if hit {
                self.0 += 1;
            }
        }
        n.visit_children_with(self);
    }

    fn visit_update_expr(&mut self, n: &UpdateExpr) {
        if is_write_to_fresh(&n.arg) {
            self.0 += 1;
        }
        n.visit_children_with(self);
    }

    // Destinos de destructuring y de for-in/of.
    fn visit_pat(&mut self, n: &Pat) {
        if let Pat::Expr(e) = n {
            if is_write_to_fresh(e) {
                self.0 += 1;
            }
        }
        n.visit_children_with(self);
    }
}

fn is_write_to_fresh(e: &Expr) -> bool {
    match e {
        Expr::Paren(p) => is_write_to_fresh(&p.expr),
        Expr::Member(m) => is_fresh(&m.obj),
        _ => false,
    }
}

/// Un literal que nadie mas referencia. Objetos solo si escribirles no puede
/// correr codigo ni cambiar el prototipo: sin getters/setters, sin `__proto__`,
/// sin claves string (podrian ser "__proto__").
fn is_fresh(e: &Expr) -> bool {
    match e {
        Expr::Paren(p) => is_fresh(&p.expr),
        Expr::Array(_) => true,
        Expr::Object(o) => o.props.iter().all(|p| match p {
            PropOrSpread::Spread(_) => true,
            PropOrSpread::Prop(p) => match &**p {
                Prop::Shorthand(i) => i.sym.as_str() != "__proto__",
                Prop::KeyValue(kv) => match &kv.key {
                    PropName::Ident(i) => i.sym.as_str() != "__proto__",
                    PropName::Str(_) => false,
                    _ => true,
                },
                Prop::Method(_) => true,
                _ => false,
            },
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{dead_writes, static_flags};
    use swc_common::{sync::Lrc, FileName, SourceMap};
    use swc_ecma_ast::{EsVersion, Program};
    use swc_ecma_parser::{parse_file_as_script, Syntax};

    fn parse(src: &str) -> Program {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(FileName::Anon.into(), src.to_string());
        let script =
            parse_file_as_script(&fm, Syntax::default(), EsVersion::latest(), None, &mut Vec::new())
                .unwrap();
        Program::Script(script)
    }

    #[test]
    fn cuenta_escrituras_sobre_literales() {
        assert_eq!(dead_writes(&parse("[1, 2][0] = 3;")), 1);
        assert_eq!(dead_writes(&parse("[[1][0], ({}).k] = [2, 3];")), 2);
        assert_eq!(dead_writes(&parse("[1][0]++; ({a: 1}).a += 1;")), 2);
        assert_eq!(dead_writes(&parse("var a = [1]; a[0] = 2; [a[0]] = [3];")), 0);
        assert_eq!(dead_writes(&parse("({ set k(v) {} }).k = 1; ({ __proto__: p }).k = 1;")), 0);
    }

    #[test]
    fn flags_estaticos() {
        assert_eq!(static_flags(&parse("function f() { return arguments.callee }")), ["callee"]);
        assert_eq!(static_flags(&parse("f.caller;")), ["callee"]);
        assert_eq!(static_flags(&parse("function g(NaN) {}")), ["redefine_global"]);
        assert!(static_flags(&parse("x === undefined; o.callee;")).is_empty());
        assert_eq!(static_flags(&parse("var o = { valueOf() { f() } };")), ["coercion"]);
        assert_eq!(static_flags(&parse("class A { toString() {} }")), ["coercion"]);
        assert_eq!(static_flags(&parse("o[Symbol.toPrimitive] = g;")), ["coercion"]);
        assert!(static_flags(&parse("x.toString(); String(y); o.valueOf();")).is_empty());
    }

    /// M13: asunciones de oxc con sensor estatico.
    #[test]
    fn flags_estaticos_de_oxc() {
        assert_eq!(static_flags(&parse("with (o) { x; }")), ["with"]);
        assert_eq!(static_flags(&parse("console.log(typeof window, document.title);")), ["global_conocido"]);
        // Declarado en el programa: no es el global.
        assert!(static_flags(&parse("var window = {}; window.x = 1;")).is_empty());
        assert!(static_flags(&parse("function f(document) { return document; }")).is_empty());
        // Existe en node (o no esta en la lista de oxc): no cuenta.
        assert!(static_flags(&parse("console.log(navigator, process, globalThis, foo);")).is_empty());
        assert_eq!(static_flags(&parse("class A extends /x/ {}")), ["extends"]);
        assert_eq!(static_flags(&parse("var B = class extends ([1]) {};")), ["extends"]);
        assert!(static_flags(&parse("class A extends null {} class C extends A {}")).is_empty());
        // La lista esta ordenada para binary_search.
        let t = super::NODE_MISSING_OXC_GLOBALS;
        assert!(t.windows(2).all(|w| w[0] < w[1]));
    }
}
