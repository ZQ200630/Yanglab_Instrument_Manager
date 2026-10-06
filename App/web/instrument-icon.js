/** Static identification art, based on the existing approximate model silhouettes.
 * No device readings, motion, GPU context, event handlers or command transport.
 * Custom boards are generic functional illustrations, not verified enclosures.
 */
const art = Object.freeze({
  aq6370: {
    title: 'AQ6370 series · Approximate appearance',
    shape: `<path d="M5 16 18 9 59 16 46 23Z" fill="#e6edf0"/>
      <path d="M46 23 59 16 59 46 46 53Z" fill="#92a5b0"/>
      <path d="M5 16 46 23 46 53 5 46Z" fill="#cad7de"/>
      <path d="M5 40 46 47 46 53 5 46Z" fill="#255783"/>
      <path d="M8 22 30 26 30 43 8 39Z" fill="#27343e"/>
      <path d="M10 24 28 27 28 40 10 37Z" fill="#17394c"/>
      <path d="m34 26 7 1v2l-7-1zm0 5 7 1v2l-7-1zm0 5 7 1v2l-7-1z" fill="#8399a6"/>
      <circle cx="38" cy="44" r="3" fill="#667d8b"/>
      <circle cx="11" cy="44" r="1.7" fill="#e8eef0"/>`,
  },
  mdt693b: {
    title: 'MDT693B · Approximate appearance',
    shape: `<path d="M5 27 19 17 61 24 48 34Z" fill="#e6edf0"/>
      <path d="M48 34 61 24 61 44 48 54Z" fill="#91a4ae"/>
      <path d="M5 27 48 34 48 54 5 47Z" fill="#d0dbdf"/>
      <path d="m8 31 9 1.5v4L8 35zm14 2 9 1.5v4L22 37zm14 2 9 1.5v4L36 39z" fill="#263740"/>
      <g fill="#344753"><circle cx="13" cy="42" r="3.2"/>
        <circle cx="27" cy="44" r="3.2"/><circle cx="41" cy="46" r="3.2"/></g>
      <path d="m13 40 0 3m14-1 0 3m14-1 0 3" stroke="#b6c7cf" stroke-width="1.2"/>
      <path d="M9 48v3m34 1v3" stroke="#556b77" stroke-width="3"/>`,
  },
  pm400: {
    title: 'PM400 · Approximate appearance',
    shape: `<path d="M17 13 27 6 50 10 40 17Z" fill="#d26469"/>
      <path d="M40 17 50 10 50 51 40 58Z" fill="#8e2b37"/>
      <path d="M17 13 40 17 40 58 17 54Z" fill="#b33d49"/>
      <path d="M20 19 37 22 37 44 20 41Z" fill="#27343e"/>
      <path d="M22 22 35 24 35 41 22 39Z" fill="#17394c"/>
      <path d="M19 44 38 47 38 55 19 52Z" fill="#e6edf0"/>
      <g fill="#697e89"><circle cx="23" cy="49" r="1"/>
        <circle cx="29" cy="50" r="1"/><circle cx="35" cy="51" r="1"/></g>
      <path d="m29 7 9 1 4-3-9-1z" fill="#8d9fa9"/>`,
  },
  'fiber-coupling': {
    title: 'NanoMax 300 · Approximate appearance',
    shape: `<path d="M5 40 28 27 60 39 36 53Z" fill="#bfcdd5"/>
      <path d="M5 40 36 53 36 59 5 46Z" fill="#8d9faa"/>
      <path d="M36 53 60 39 60 45 36 59Z" fill="#637c8b"/>
      <path d="M12 29 33 17 53 25 32 38Z" fill="#40545f"/>
      <path d="M12 29 32 38 32 50 12 41Z" fill="#263740"/>
      <path d="M32 38 53 25 53 38 32 50Z" fill="#172932"/>
      <path d="m13 34 18 8m-18-3 18 8m3-7 18-11m-18 16 18-11" stroke="#8fa4af" stroke-width="1"/>
      <path d="M22 23 35 16 45 20 32 28Z" fill="#cbd8de"/>
      <path d="M32 28 45 20 45 24 32 32 22 27 22 23Z" fill="#7c929f"/>
      <path d="M29 17v-7l7-4 5 2v11l-7 4Z" fill="#b0c1ca"/>
      <path d="m34 6 7 2v11l-7 4Z" fill="#657e8d"/>
      <path d="m40 15 17 7" stroke="#b79a55" stroke-width="2"/>
      <path d="m12 39-6 3m22 4-5 7m25-12 8 4" stroke="#abbec9" stroke-width="4"/>
      <g fill="#344a57"><ellipse cx="5" cy="44" rx="3" ry="4"/>
        <ellipse cx="22" cy="54" rx="3" ry="4"/><ellipse cx="57" cy="46" rx="3" ry="4"/></g>`,
  },
  voltage: {
    title: 'Generic 8-channel voltage source · Appearance unverified',
    shape: `<path d="M4 30 25 16 61 30 40 46Z" fill="#5b9472"/>
      <path d="M4 30 40 46 40 50 4 34Z" fill="#2e6b4e"/>
      <path d="M40 46 61 30 61 34 40 50Z" fill="#24583f"/>
      <path d="m24 20 8 3-5 4-8-3Z" fill="#293e46"/>
      <path d="m7 27 6-4 5 2-6 5Z" fill="#bccbd2"/>
      ${[[14, 31], [24, 35], [34, 39], [44, 43], [23, 24], [33, 28], [43, 32], [53, 36]].map(([x, y]) =>
        `<g transform="translate(${x} ${y})"><path d="m-5-6 5-3 7 3-5 3Z" fill="#5592b8"/>
          <path d="m-5-6 7 3v5l-7-3Z" fill="#255783"/>
          <path d="m2-3 5-3v5L2 2Z" fill="#1a4267"/>
          <circle cy="-6" r="1" fill="#d5e2e9"/></g>`).join('')}`,
  },
  gain: {
    title: 'Generic Gain Chip Driver · Appearance unverified',
    shape: `<path d="M4 30 25 16 61 30 40 46Z" fill="#5b9472"/>
      <path d="M4 30 40 46 40 50 4 34Z" fill="#2e6b4e"/>
      <path d="M40 46 61 30 61 34 40 50Z" fill="#24583f"/>
      <path d="m24 20 8 3-5 4-8-3Z" fill="#293e46"/>
      <path d="m7 27 6-4 5 2-6 5Z" fill="#bccbd2"/>
      <path d="m11 27 9-6 13 5-9 7v7l-13-5Zm24 9 9-6 13 5-9 7v7l-13-5Z" fill="#586d7a"/>
      <path d="m13 24 0 9 12 5 0-9Zm4-3 0 9 12 5 0-9Zm4-3 0 9 12 5 0-9Z
        m15 15 0 9 12 5 0-9Zm4-3 0 9 12 5 0-9Zm4-3 0 9 12 5 0-9Z" fill="#c1d0d8"/>
      <path d="m8 35 6-3 9 4v5l-9-4-6 3Zm22 9 6-3 9 4v5l-9-4-6 3Z" fill="#255783"/>`,
  },
  instrument: {
    title: 'Generic instrument · Identification only',
    shape: `<path d="M8 22 23 13 57 22 42 32Z" fill="#dbe5ea"/>
      <path d="M8 22 42 32 42 54 8 44Z" fill="#acbec9"/>
      <path d="M42 32 57 22 57 44 42 54Z" fill="#7d95a3"/>
      <path d="m13 29 16 5v10l-16-5Z" fill="#344e5e"/>
      <circle cx="35" cy="44" r="3" fill="#e7edf0"/>`,
  },
});

export function renderInstrumentIcon(modelId) {
  const key = Object.hasOwn(art, modelId) ? modelId : 'instrument';
  const {title, shape} = art[key];
  return `<svg class="instrument-icon" data-instrument-icon="${key}" width="48" height="48"
    viewBox="0 0 64 64" aria-hidden="true" focusable="false" xmlns="http://www.w3.org/2000/svg">
    <title>${title}</title><ellipse cx="32" cy="56" rx="28" ry="5" fill="#e8eef1"/>${shape}</svg>`;
}
