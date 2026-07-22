+++
name = "coordinates-as-interface"
domain = "concepts / representation"
triggers = ["coordinate system", "reference frame", "spherical geometry", "geometry as interface"]
rank = 0.6
+++
A coordinate system is an interface between an observer and a space, not a
commitment to the space's true shape — a lens chosen to make certain
operations easy, not a discovery of what's "really there." Same point,
different chart (Cartesian vs. polar), and the math for the same operation
(e.g. translation) goes from trivial to a trigonometric mess purely from the
choice of representation, not the underlying geometry.

Practical carry-over for this codebase: when a data structure feels awkward
to manipulate, suspect the *coordinate choice* before the *data* — e.g. the
brain-ring's `[i8; 3]` layer coordinates and `project()`'s polar-style
angle/radius placement (`crates/viz/src/brain.rs`) are themselves one lens
on the underlying graph; a different projection could make different
relationships "sharp" at the cost of others going distorted. Don't mistake
a representation's convenience for a claim about the thing represented.

Source: "Geometry as Computational Infrastructure" (independent researcher
Flyxion, 2026) — via github.com/standardgalactic/kitbash, an unrelated
personal research repo (audio transcripts + LaTeX draft, not code) surfaced
2026-07-22. Nothing else in that repo is applicable here.
