// Phone stand: a minimal, parametric example for OpenSuperCAD.
// Open this folder as a project and ask your agent to change it, e.g.
// "make it fit a tablet and add a cable slot".

/* [Phone] */
// Thickness of the phone including its case (mm)
phone_thickness = 11; // [6:0.5:16]
// Viewing angle from horizontal (degrees)
angle = 70; // [45:85]

/* [Stand] */
// Width of the stand (mm)
width = 70; // [40:120]
// Depth of the base (mm)
depth = 80; // [50:120]
// Wall / plate thickness (mm)
wall = 4; // [2:0.5:8]
// Add a hole for a charging cable
cable_hole = true;

/* [Hidden] */
$fn = 48;
eps = 0.01;

module base() {
  cube([width, depth, wall]);
}

module back_rest() {
  translate([0, depth * 0.35, 0])
    rotate([angle - 90, 0, 0])
      cube([width, wall, depth * 1.1]);
}

module lip() {
  translate([0, depth * 0.35 - phone_thickness - wall, 0])
    cube([width, wall, wall + 8]);
}

difference() {
  union() {
    base();
    back_rest();
    lip();
  }
  if (cable_hole)
    translate([width / 2, depth * 0.35 - phone_thickness / 2, -eps])
      cylinder(d = 10, h = wall + 2 * eps);
}
