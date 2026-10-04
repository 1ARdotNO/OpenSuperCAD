// Twisted polygonal vase, printable in vase mode or with solid walls.

/* [Shape] */
// Height in mm
height = 120;        // [40:5:250]
// Base radius in mm
radius = 35;         // [15:1:80]
// Number of sides
sides = 7;           // [3:1:16]
// Total twist in degrees
twist = 120;         // [0:10:360]
// Top scale relative to the base
flare = 1.3;         // [0.5:0.05:2]
// Round the corners
rounded = true;

/* [Walls] */
// Wall thickness in mm (0 for a solid body, to print in vase mode)
wall = 2;            // [0:0.2:5]
// Base thickness in mm
floor_thickness = 3; // [1:0.5:10]

/* [Hidden] */
$fn = 48;
slices = 60;

module profile(r) {
    if (rounded)
        offset(r = r * 0.15) circle(r = r * 0.85, $fn = sides);
    else
        circle(r = r, $fn = sides);
}

module body(r) {
    linear_extrude(height = height, twist = twist, scale = flare, slices = slices)
        profile(r);
}

color("teal")
difference() {
    body(radius);
    // The cavity uses the same extrusion as the outside, so the wall follows
    // the twist exactly; only the floor is cut off.
    if (wall > 0)
        difference() {
            translate([0, 0, 0.01]) body(radius - wall);
            translate([-radius * 3, -radius * 3, 0]) cube([radius * 6, radius * 6, floor_thickness]);
        }
}
