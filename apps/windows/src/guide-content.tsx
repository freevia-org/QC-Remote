/**
 * The user guide's body: a section list down the left and the sections, with
 * their shortcut tables, beside it.
 *
 * The content is the data module in `user-guide.ts`; this file is only how it
 * is laid out. The User Guide window renders it as the whole of its page.
 */

import { useRef, useState } from "react";
import { USER_GUIDE, type GuideBlock } from "./user-guide";

function GuideBlockView({ block }: { block: GuideBlock }) {
  if (block.kind === "paragraph") return <p>{block.text}</p>;
  if (block.kind === "note") return <p className="guide-note">{block.text}</p>;
  if (block.kind === "steps") return <ol>{block.items.map((item) => <li key={item}>{item}</li>)}</ol>;
  if (block.kind === "list") return <ul>{block.items.map((item) => <li key={item}>{item}</li>)}</ul>;
  return <table className="shortcut-table">
    <thead><tr><th>Keys</th><th>Action</th></tr></thead>
    <tbody>{block.rows.map((row) => <tr key={row.keys}><td>{row.keys}</td><td>{row.action}</td></tr>)}</tbody>
  </table>;
}

export function GuideContent() {
  const [guideSection, setGuideSection] = useState(USER_GUIDE[0].id);
  const headings = useRef(new Map<string, HTMLHeadingElement | null>());

  const showGuideSection = (id: string) => {
    setGuideSection(id);
    headings.current.get(id)?.scrollIntoView({ block: "start" });
  };

  return <div className="guide-layout">
    <nav className="guide-nav" aria-label="User guide sections">
      {USER_GUIDE.map((section) => <button
        key={section.id}
        type="button"
        aria-current={guideSection === section.id ? "true" : undefined}
        className={guideSection === section.id ? "is-active" : undefined}
        onClick={() => showGuideSection(section.id)}
      >{section.title}</button>)}
    </nav>
    <div className="guide-body">
      {USER_GUIDE.map((section) => <section key={section.id}>
        <h3 id={`guide-${section.id}`} ref={(node) => { headings.current.set(section.id, node); }}>{section.title}</h3>
        <p className="guide-lead">{section.summary}</p>
        {section.blocks.map((block, index) => <GuideBlockView key={index} block={block} />)}
      </section>)}
    </div>
  </div>;
}
