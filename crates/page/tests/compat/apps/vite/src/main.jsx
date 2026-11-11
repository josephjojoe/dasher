// A Vite-built Preact app: module entry, a vendor chunk, and a route
// component split into its own chunk with its own CSS.
import { render } from "preact";
import { useEffect, useState } from "preact/hooks";
import { formatDate } from "./format.js";
import "./style.css";

const base = import.meta.env.BASE_URL;

function Story({ story }) {
  return (
    <li class="story" key={story.id}>
      <a href={story.url}>{story.title}</a>
      <span class="meta">
        {story.points} points · {formatDate(story.date)}
      </span>
    </li>
  );
}

function App() {
  const [stories, setStories] = useState([]);
  const [Comments, setComments] = useState(null);
  useEffect(() => {
    fetch(`${base}api/stories.json`)
      .then((r) => r.json())
      .then((data) => setStories(data.stories));
    // Lazily loaded route chunk, as with React.lazy/defineAsyncComponent.
    import("./routes/Comments.jsx").then((module) => setComments(() => module.default));
  }, []);
  return (
    <main>
      <h1>Top stories</h1>
      <ol id="stories">
        {stories.map((story) => (
          <Story story={story} />
        ))}
      </ol>
      {Comments ? <Comments /> : <p>Loading comments…</p>}
    </main>
  );
}

render(<App />, document.getElementById("app"));
