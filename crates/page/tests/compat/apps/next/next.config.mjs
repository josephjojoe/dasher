// A static export, served under /compat/next by the fixture server.
export default {
  output: "export",
  basePath: "/compat/next",
  images: { unoptimized: true },
  // Keep the build reproducible: the same id every time.
  generateBuildId: () => "compat",
};
