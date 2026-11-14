// Rewrites the game's touch-control instructions so they name keyboard & mouse
// keys instead. The engine applies these to the game's own string tables, so
// the text is drawn by the game in its own fonts and styling.
//
// Rule forms (English text only; other languages are left untouched):
//   [from, to]          replace every occurrence of `from`
//   ["=" + from, to]    replace a whole string that is exactly `from`

const PICKUPS = [
  "MN106", "AK-47", "S1 Custom", "RPG-7", "MP5", "M40A3", "M249", "Beretta M9", "Desert Eagle",
  "MN106 Silenced", "Beretta M9 Silenced", "MAC11", "MAC11 Silenced", "Dradonitch", "Benelli M4",
  "MP5 Silenced", "AK47 Gold", "AK47 Silenced", "MN106 Red Dot", "MN106 R.D.S.", "MN106 Camo",
  "MN106 Camo R.D.", "MN106 C.R.D.S.", "Golden Pistol", "NX8 Handgun", "Vulture", "SPAM-4", "NX8 Silenced",
];

export function promptRules(k) {
  // Highlight keys the way the game's own hints highlight controls.
  const K = (s) => `<font color='#FF3300'>${s}</font>`;
  const up = (s) => s.toUpperCase();
  return [
    // Tutorial hints (they support colour markup).
    ["<b>Stand up</b> by tapping the <font color='#FF3300'>crouch icon</font> and <b>crouch</b> by tapping the <font color='#FF3300'>stand icon</font>.",
      `Press ${K(k.crouch)} to <b>crouch</b> or <b>stand up</b>.`],
    ["Tap the <font color='#FF3300'>melee icon</font> to attack", `Press ${K(k.melee)} to attack`],
    ["Tap the <font color='#FF3300'>shoot icon</font> to fire the pistol.", `Press the ${K(k.fire)} to fire the pistol.`],
    ["Tap the<font color='#FF3300'> iron sight icon</font> to shoot", `Press the ${K(k.aim)} to toggle aiming`],
    ["Follow Downs' gun by <font color='#FF3300'>using the virtual stick or dragging on the screen</font>.", `${K("Click")} to take control of the mouse, then follow Downs' gun by moving it.`],
    ["Use the <font color='#FF3300'>Virtual Analog Stick</font> to move around the cell.", `Use ${K(k.move)} to move around the cell.`],
    ["by tapping the weapon icon when it appears.", `by pressing ${K(k.use)} when the weapon icon appears.`],
    ["Reload your gun by <b>tapping</b> the <font color='#FF3300'>weapon icon</font> on the upper right of the screen.", `Reload your gun by pressing ${K(k.reload)}.`],
    ["Switch weapons by <b>tapping twice</b> on the <font color='#FF3300'>weapon icon</font> or <b>dragging</b> across it.", `Switch weapons by pressing ${K(k.switchWeapon)} or using the ${K("mouse wheel")}.`],
    ["You can walk slowly by moving the Virtual Analog Stick <font color='#FF3300'>slightly forward</font>.", `You can walk slowly by holding ${K(k.walk)} while moving.`],
    ["Tap the <font color='#FF3300'>grenade icon</font> to throw one.", `Press ${K(k.grenade)} to throw one.`],
    ["<font color='#FF3300'>Double tap the grenade icon</font> to select a flash grenade.", `Press ${K(k.switchGrenade)} to select a flash grenade.`],
    ["Tap the <font color='#FF3300'>turret icon</font> to man", `Press ${K(k.use)} to man`],
    ["tap the <font color='#FF3300'>airstrike icon</font>.", `press ${K(k.airstrike)}.`],

    // Weapon guide in the menus (plain upper-case text).
    ["TO THROW A HAND GRENADE SIMPLY TAP THE GRENADE ICON ON THE HUD.\nTO USE A FLASHBANG, DOUBLE TAP THE GRENADE ICON TO SWITCH TO THE FLASHBANG ICON.",
      `TO THROW A HAND GRENADE PRESS ${up(k.grenade)}.\nTO USE A FLASHBANG, PRESS ${up(k.switchGrenade)} TO SWITCH TO THE FLASHBANG.`],
    ["TO SIGNAL AN AIRSTRIKE, TAP THE AIRSTRIKE ICON ON YOUR HUD WHEN THIS ABILITY IS AVAILABLE.",
      `TO SIGNAL AN AIRSTRIKE, PRESS ${up(k.airstrike)} WHEN THIS ABILITY IS AVAILABLE.`],
    ["AND TAP THE MELEE ICON WHEN IT APPEARS ON YOUR HUD.", `AND PRESS ${up(k.melee)} WHEN THE MELEE ICON APPEARS ON YOUR HUD.`],
    ["YOU CAN EITHER DOUBLE TAP OR DRAG ACROSS THE WEAPON ICON IN THE TOP RIGHT CORNER OF THE IN-GAME HUD TO SWITCH WEAPONS.",
      `PRESS ${up(k.switchWeapon)} OR USE THE MOUSE WHEEL TO SWITCH WEAPONS. PRESS ${up(k.reload)} TO RELOAD.`],

    // Options -> Controls: every scheme description becomes the keyboard &
    // mouse reference (the touch schemes don't apply).
    ...["DRAG YOUR FINGER ALONG THE SCREEN TO CHANGE VIEWING ANGLE.\nDRAG THE STICK IN THE BOTTOM-LEFT CORNER TO MOVE.\nTAP THE BOTTOM-RIGHT ICON TO FIRE.",
      "DRAG THE RIGHT SIDE OF THE SCREEN TO CHANGE VIEWING ANGLE.\nDRAG THE LEFT SIDE OF THE SCREEN TO MOVE.\nDOUBLE TAP ANYWHERE ON THE SCREEN TO SHOOT.",
      "DRAG THE STICK IN THE BOTTOM-RIGHT CORNER TO CHANGE VIEWING ANGLE.\nDRAG THE STICK IN THE BOTTOM-LEFT CORNER TO MOVE.\nTAP ANYWHERE ELSE ON THE SCREEN TO SHOOT.",
      "IN THIS MODE, CONTROLS CAN BE ARRANGED ON THE HUD ACCORDING TO THE PLAYER'S PREFERENCES. DRAG THE CONTROLS TO ANY LOCATION ON THE SCREEN AND SAVE THE SETTINGS.",
    ].map((t) => ["=" + t, keyboardReference(k)]),

    // Title screen.
    ["=TOUCH THE SCREEN TO CONTINUE", "CLICK TO CONTINUE"],
    ["=Touch to play", "Click to play"],
    ["=TOUCH ", "CLICK "],

    // In-world pickup prompts ("Pick up NX8 Handgun").
    ...PICKUPS.map((w) => [`=Pick up ${w}`, `Press ${k.use} to pick up ${w}`]),
  ];
}

function keyboardReference(k) {
  const u = (s) => s.toUpperCase();
  return [
    `CLICK THE SCREEN TO TAKE CONTROL OF THE MOUSE, ${u(k.pause)} TO PAUSE. MOVE THE MOUSE TO LOOK AROUND.`,
    `${u(k.move)}: MOVE   ${u(k.walk)}: WALK   ${u(k.crouch)}: CROUCH`,
    `${u(k.fire)}: FIRE   ${u(k.aim)}: TOGGLE AIM   ${u(k.reload)}: RELOAD   ${u(k.switchWeapon)} / MOUSE WHEEL: SWITCH WEAPON`,
    `${u(k.grenade)}: GRENADE   ${u(k.switchGrenade)}: GRENADE TYPE   ${u(k.use)}: USE   ${u(k.melee)}: MELEE   ${u(k.airstrike)}: AIRSTRIKE   ${u(k.skip)}: SKIP CUTSCENE`,
  ].join("\n");
}
