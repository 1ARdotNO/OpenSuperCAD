// Parametric box with lid, showing customizer groups, dropdowns and sliders.

/* [Size] */
// Inner width (mm)
inner_width = 60; // [20:1:200]
// Inner depth (mm)
inner_depth = 40; // [20:1:200]
// Inner height (mm)
inner_height = 25; // [10:1:150]

/* [Style] */
// Wall thickness (mm)
wall = 2; // [1.2:0.4:5]
// Corner style
corners = "round"; // [round:Rounded, sharp:Sharp]
// Corner radius when rounded (mm)
radius = 4; // [1:10]
// Which parts to show
part = "both"; // [both:Box and lid, box:Box only, lid:Lid only]

/* [Hidden] */
$fn = 40;
eps = 0.01;
gap = 0.3;

module rounded_rect(size, r) {
  if (corners == "round")
    offset(r = r) offset(delta = -r) square(size);
  else
    square(size);
}

module shell(w, d, h) {
  linear_extrude(h) rounded_rect([w, d], radius);
}

module box() {
  w = inner_width + 2 * wall;
  d = inner_depth + 2 * wall;
  difference() {
    shell(w, d, inner_height + wall);
    translate([wall, wall, wall]) shell(inner_width, inner_depth, inner_height + eps);
  }
}

module lid() {
  w = inner_width + 2 * wall;
  d = inner_depth + 2 * wall;
  translate([0, d + 10, 0]) {
    shell(w, d, wall);
    translate([wall + gap, wall + gap, wall - eps])
      difference() {
        shell(inner_width - 2 * gap, inner_depth - 2 * gap, 4);
        translate([wall, wall, -eps]) shell(inner_width - 2 * gap - 2 * wall, inner_depth - 2 * gap - 2 * wall, 4 + 2 * eps);
      }
  }
}

if (part != "lid") box();
if (part != "box") lid();
