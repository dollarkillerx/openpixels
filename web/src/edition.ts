/**
 * Which edition this build is. The browser edition matches Lightroom Classic's feature
 * set; everything beyond that belongs to the full desktop edition.
 */
export const IS_LITE = import.meta.env.MODE === "lite";
