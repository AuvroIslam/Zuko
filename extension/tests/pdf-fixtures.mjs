// Hand-built minimal PDFs for the tests (valid xref table, Helvetica text, one content stream per page).

const esc = (s) => s.replace(/\\/g, "\\\\").replace(/\(/g, "\\(").replace(/\)/g, "\\)");

/** pages: array of arrays of lines ([] = a page with no text, like a scanned page). */
export function makePdf(pages) {
  const n = pages.length;
  const objects = new Map();
  objects.set(1, "<< /Type /Catalog /Pages 2 0 R >>");
  objects.set(2, `<< /Type /Pages /Kids [${pages.map((_, i) => `${4 + i * 2} 0 R`).join(" ")}] /Count ${n} >>`);
  objects.set(3, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
  pages.forEach((lines, i) => {
    const page = 4 + i * 2;
    const content = page + 1;
    objects.set(page, `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents ${content} 0 R /Resources << /Font << /F1 3 0 R >> >> >>`);
    const stream = lines.length ? `BT /F1 12 Tf 72 720 Td 16 TL ${lines.map((l) => `(${esc(l)}) Tj T*`).join(" ")} ET` : "";
    objects.set(content, `<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`);
  });
  let out = "%PDF-1.4\n";
  const offsets = [];
  for (let id = 1; id <= 3 + n * 2; id++) {
    offsets[id] = out.length;
    out += `${id} 0 obj\n${objects.get(id)}\nendobj\n`;
  }
  const xref = out.length;
  const count = 4 + n * 2;
  out += `xref\n0 ${count}\n0000000000 65535 f \n`;
  for (let id = 1; id < count; id++) out += `${String(offsets[id]).padStart(10, "0")} 00000 n \n`;
  out += `trailer\n<< /Size ${count} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return new Uint8Array(Buffer.from(out, "latin1"));
}
