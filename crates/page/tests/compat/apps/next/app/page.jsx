// A server component: rendered to HTML at build time, with its RSC payload
// inlined for hydration.
import Link from "next/link";
import Likes from "./likes";
import Reviews from "./reviews";

const article = {
  title: "Hydration in a headless browser",
  author: "Ada Lovelace",
  paragraphs: [
    "Server-rendered markup arrives with the document.",
    "The client bundle then attaches event handlers without re-rendering it.",
  ],
};

export default function Page() {
  return (
    <main>
      <nav>
        <Link id="about-link" href="/about">
          About
        </Link>
      </nav>
      <article>
        <h1 id="headline">{article.title}</h1>
        <p className="byline">By {article.author}</p>
        {article.paragraphs.map((text) => (
          <p key={text} className="body">
            {text}
          </p>
        ))}
        <Likes initial={41} />
      </article>
      <Reviews />
    </main>
  );
}
