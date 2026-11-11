// A Pages Router page: HTML and __NEXT_DATA__ from getStaticProps, then
// hydrated by Next's pages runtime.
import Head from "next/head";
import { useEffect, useState } from "react";
import { useRouter } from "next/router";

export async function getStaticProps() {
  return {
    props: {
      products: [
        { id: 1, name: "Teapot" },
        { id: 2, name: "Kettle" },
      ],
    },
  };
}

export default function Legacy({ products }) {
  const router = useRouter();
  const [stock, setStock] = useState(null);
  useEffect(() => {
    fetch("/compat/next/api/stock.json")
      .then((r) => r.json())
      .then(setStock);
  }, []);
  return (
    <>
      <Head>
        <title>Legacy shop</title>
      </Head>
      <h1 id="legacy-title">Shop ({router.pathname})</h1>
      <ul id="products">
        {products.map((p) => (
          <li key={p.id}>
            {p.name}
            {stock && <span className="stock"> ({stock[p.id]} in stock)</span>}
          </li>
        ))}
      </ul>
    </>
  );
}
