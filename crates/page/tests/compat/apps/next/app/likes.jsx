"use client";
import { useEffect, useState } from "react";

export default function Likes({ initial }) {
  const [likes, setLikes] = useState(initial);
  const [hydrated, setHydrated] = useState(false);
  useEffect(() => setHydrated(true), []);
  return (
    <button
      type="button"
      id="like"
      data-hydrated={hydrated ? "true" : "false"}
      onClick={() => setLikes((n) => n + 1)}
    >
      {likes} likes
    </button>
  );
}
