// History routing with React Router's data router: the server answers every
// path under /compat/router/ with the same document, and the router renders
// the route for location.pathname, loading its data first.
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import {
  Link,
  NavLink,
  Outlet,
  RouterProvider,
  createBrowserRouter,
  useLoaderData,
  useNavigation,
  useParams,
  useSearchParams,
} from "react-router";

const api = (path) => `/compat/router/api/${path}`;

async function getJson(path, request) {
  const response = await fetch(api(path), { signal: request.signal });
  if (!response.ok) throw new Response("Not found", { status: response.status });
  return response.json();
}

function Layout() {
  const navigation = useNavigation();
  return (
    <div id="layout" data-navigation={navigation.state}>
      <nav>
        <NavLink to="/" end>
          Catalogue
        </NavLink>{" "}
        <NavLink to="/about">About</NavLink>
      </nav>
      <Outlet />
    </div>
  );
}

function Catalogue() {
  const { items } = useLoaderData();
  const [params] = useSearchParams();
  const sort = params.get("sort") ?? "name";
  const sorted = [...items].sort((a, b) =>
    sort === "price" ? a.price - b.price : a.name.localeCompare(b.name),
  );
  return (
    <section id="catalogue" data-sort={sort}>
      <h1>Catalogue</h1>
      <ul>
        {sorted.map((item) => (
          <li key={item.id}>
            <Link className="item-link" to={`/items/${item.id}`}>
              {item.name}
            </Link>
          </li>
        ))}
      </ul>
    </section>
  );
}

function Item() {
  const item = useLoaderData();
  const { id } = useParams();
  return (
    <section id="item" data-id={id}>
      <h1>{item.name}</h1>
      <p className="description">{item.description}</p>
      <p className="price">£{item.price}</p>
      <Link to="/">Back to the catalogue</Link>
    </section>
  );
}

function About() {
  return (
    <section id="about">
      <h1>About</h1>
      <p>A catalogue rendered by history routing.</p>
    </section>
  );
}

function NotFound() {
  return <h1 id="not-found">Page not found</h1>;
}

const router = createBrowserRouter(
  [
    {
      path: "/",
      element: <Layout />,
      errorElement: <NotFound />,
      children: [
        { index: true, element: <Catalogue />, loader: ({ request }) => getJson("items.json", request) },
        {
          path: "items/:id",
          element: <Item />,
          loader: ({ params, request }) => getJson(`items/${params.id}.json`, request),
        },
        { path: "about", element: <About /> },
      ],
    },
  ],
  { basename: "/compat/router" },
);

window.__router = router;

createRoot(document.getElementById("root")).render(
  <StrictMode>
    <RouterProvider router={router} />
  </StrictMode>,
);
