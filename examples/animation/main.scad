// Animated example: a crank driving a slider. Press "Animate" in the preview,
// or ask your agent to "check the mechanism at t = 0.25 and 0.75".

/* [Mechanism] */
// Crank radius (mm)
crank = 15; // [5:40]
// Connecting rod length (mm)
rod = 45; // [30:100]

/* [Hidden] */
$fn = 32;
angle = 360 * $t;

crank_pin = [crank * cos(angle), crank * sin(angle)];
slider_x = crank * cos(angle) + sqrt(rod * rod - pow(crank * sin(angle), 2));

// Base plate
translate([-crank - 10, -10, -4]) cube([crank + rod + 40, 20, 2]);
// Crank
color("SteelBlue") hull() {
  cylinder(r = 4, h = 3);
  translate(crank_pin) cylinder(r = 3, h = 3);
}
// Connecting rod
color("Orange") translate([0, 0, 3]) hull() {
  translate(crank_pin) cylinder(r = 2.5, h = 2);
  translate([slider_x, 0]) cylinder(r = 2.5, h = 2);
}
// Slider block
color("Tomato") translate([slider_x - 6, -6, -2]) cube([12, 12, 8]);
