package sanitize

// Element names ammonia 4 treats as SVG and MathML when deciding whether a
// namespace switch is legitimate.

var svgTagNames = tagSet(
	"a", "animate", "animateColor", "animateMotion", "animateTransform", "circle", "clipPath", "defs",
	"desc", "discard", "ellipse", "feBlend", "feColorMatrix", "feComponentTransfer", "feComposite",
	"feConvolveMatrix", "feDiffuseLighting", "feDisplacementMap", "feDistantLight", "feDropShadow",
	"feFlood", "feFuncA", "feFuncB", "feFuncG", "feFuncR", "feGaussianBlur", "feImage", "feMerge",
	"feMergeNode", "feMorphology", "feOffset", "fePointLight", "feSpecularLighting", "feSpotLight",
	"feTile", "feTurbulence", "filter", "foreignObject", "g", "image", "line", "linearGradient",
	"marker", "mask", "metadata", "mpath", "path", "pattern", "polygon", "polyline", "radialGradient",
	"rect", "script", "set", "stop", "style", "svg", "switch", "symbol", "text", "textPath", "title",
	"tspan", "use", "view",
)

var mathmlTagNames = tagSet(
	"abs", "and", "annotation", "annotation-xml", "apply", "approx", "arccos", "arccosh", "arccot",
	"arccoth", "arccsc", "arccsch", "arcsec", "arcsech", "arcsin", "arcsinh", "arctan", "arctanh",
	"arg", "bind", "bvar", "card", "cartesianproduct", "cbytes", "ceiling", "cerror", "ci", "cn",
	"codomain", "complexes", "compose", "condition", "conjugate", "cos", "cosh", "cot", "coth", "cs",
	"csc", "csch", "csymbol", "curl", "declare", "degree", "determinant", "diff", "divergence",
	"divide", "domain", "domainofapplication", "emptyset", "eq", "equivalent", "eulergamma", "exists",
	"exp", "exponentiale", "factorial", "factorof", "false", "floor", "fn", "forall", "gcd", "geq",
	"grad", "gt", "ident", "image", "imaginary", "imaginaryi", "implies", "in", "infinity", "int",
	"integers", "intersect", "interval", "inverse", "lambda", "laplacian", "lcm", "leq", "limit",
	"list", "ln", "log", "logbase", "lowlimit", "lt", "maction", "maligngroup", "malignmark", "math",
	"matrix", "matrixrow", "max", "mean", "median", "menclose", "merror", "mfenced", "mfrac",
	"mglyph", "mi", "min", "minus", "mlabeledtr", "mlongdiv", "mmultiscripts", "mn", "mo", "mode",
	"moment", "momentabout", "mover", "mpadded", "mphantom", "mprescripts", "mroot", "mrow", "ms",
	"mscarries", "mscarry", "msgroup", "msline", "mspace", "msqrt", "msrow", "mstack", "mstyle",
	"msub", "msubsup", "msup", "mtable", "mtd", "mtext", "mtr", "munder", "munderover",
	"naturalnumbers", "neq", "none", "not", "notanumber", "notin", "notprsubset", "notsubset", "or",
	"otherwise", "outerproduct", "partialdiff", "pi", "piece", "piecewise", "plus", "power", "primes",
	"product", "prsubset", "quotient", "rationals", "real", "reals", "reln", "rem", "root",
	"scalarproduct", "sdev", "sec", "sech", "selector", "semantics", "sep", "set", "setdiff", "share",
	"sin", "sinh", "span", "subset", "sum", "tan", "tanh", "tendsto", "times", "transpose", "true",
	"union", "uplimit", "variance", "vector", "vectorproduct", "xor",
)
