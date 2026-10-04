// Parametric spur gear with an approximated involute profile and a keyed bore.

/* [Gear] */
// Number of teeth
teeth = 24;           // [8:1:80]
// Module (pitch diameter / teeth) in mm
gear_module = 2;      // [0.5:0.25:5]
// Face width in mm
thickness = 8;        // [2:0.5:30]
// Pressure angle in degrees
pressure_angle = 20;  // [14.5, 20, 25]

/* [Hub] */
// Bore diameter in mm
bore = 8;             // [2:0.5:30]
// Add a keyway to the bore
keyway = true;
// Number of lightening holes
holes = 6;            // [0:1:12]

/* [Hidden] */
$fn = 64;

pitch_r = teeth * gear_module / 2;
outer_r = pitch_r + gear_module;
root_r = pitch_r - 1.25 * gear_module;
base_r = pitch_r * cos(pressure_angle);

// Point on the involute of a circle of radius r at roll angle t (degrees).
function involute(r, t) = r * [cos(t) + t * PI / 180 * sin(t), sin(t) - t * PI / 180 * cos(t)];

function roll_at(r) = sqrt(max(0, (r / base_r) ^ 2 - 1)) * 180 / PI;

module tooth() {
    steps = 8;
    t_max = roll_at(outer_r);
    // Half tooth thickness at the pitch circle, as an angle.
    half = 90 / teeth + (tan(pressure_angle) - pressure_angle * PI / 180) * 180 / PI;
    flank = [for (i = [0:steps]) involute(base_r, t_max * i / steps)];
    rot = function(p, a) [p.x * cos(a) - p.y * sin(a), p.x * sin(a) + p.y * cos(a)];
    left = [for (p = flank) rot(p, -half)];
    right = [for (i = [steps:-1:0]) let(p = flank[i]) rot([p.x, -p.y], half)];
    polygon(concat([[0, 0]], left, right));
}

module gear_2d() {
    union() {
        circle(r = root_r);
        for (i = [0:teeth - 1]) rotate(i * 360 / teeth) tooth();
    }
}

difference() {
    linear_extrude(height = thickness) gear_2d();
    translate([0, 0, -1]) cylinder(d = bore, h = thickness + 2);
    if (keyway)
        translate([bore / 2 - 0.5, -bore / 8, -1]) cube([bore / 4 + 0.5, bore / 4, thickness + 2]);
    if (holes > 0 && root_r - bore > 12)
        for (i = [0:holes - 1])
            rotate(i * 360 / holes + 180 / holes)
                translate([(root_r + bore / 2) / 2, 0, -1])
                    cylinder(d = (root_r - bore / 2) * 0.38, h = thickness + 2);
}
