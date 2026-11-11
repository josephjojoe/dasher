"use client";
import { useEffect, useState } from "react";
import { usePathname } from "next/navigation";

// Client-only data, fetched after hydration.
export default function Reviews() {
  const pathname = usePathname();
  const [reviews, setReviews] = useState(null);
  useEffect(() => {
    let cancelled = false;
    (async () => {
      const response = await fetch("/compat/next/api/reviews.json");
      const body = await response.json();
      if (!cancelled) setReviews(body.reviews);
    })();
    return () => {
      cancelled = true;
    };
  }, []);
  if (reviews === null) return <p id="reviews-loading">Loading reviews…</p>;
  return (
    <ol id="reviews" data-pathname={pathname}>
      {reviews.map((review) => (
        <li key={review.id} className="review">
          <strong>{review.author}</strong>: {review.text}
        </li>
      ))}
    </ol>
  );
}
