import { useEffect, useState } from "preact/hooks";
import "./comments.css";

export default function Comments() {
  const [comments, setComments] = useState([]);
  useEffect(() => {
    fetch(new URL(`${import.meta.env.BASE_URL}api/comments.json`, location.origin))
      .then((r) => r.json())
      .then((data) => setComments(data.comments));
  }, []);
  return (
    <section id="comments">
      <h2>Comments</h2>
      {comments.map((c) => (
        <p class="comment">{c}</p>
      ))}
    </section>
  );
}
