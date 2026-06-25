import NextAuth, { type DefaultSession } from "next-auth";

declare module "next-auth" {
  interface Session {
    access_token?: string;
    user: DefaultSession["user"] & {
      id?: string;
    };
  }
}

const issuer = `${process.env.NEXT_PUBLIC_KEYCLOAK_URL}/realms/${process.env.NEXT_PUBLIC_KEYCLOAK_REALM}`;

export const { handlers, signIn, signOut, auth } = NextAuth({
  trustHost: true,
  secret: process.env.AUTH_SECRET,
  providers: [
    {
      id: "keycloak",
      name: "Keycloak",
      type: "oidc",
      issuer,
      clientId: process.env.NEXT_PUBLIC_KEYCLOAK_CLIENT_ID,
      clientSecret: process.env.KEYCLOAK_FRONTEND_CLIENT_SECRET ?? "",
      wellKnown: `${issuer}/.well-known/openid-configuration`,
      authorization: {
        params: { scope: "openid email profile" },
      },
      idToken: true,
    },
  ],
  callbacks: {
    async jwt({ token, account }) {
      if (account?.access_token) {
        token.access_token = account.access_token;
      }
      return token;
    },
    async session({ session, token }) {
      if (token.access_token) {
        session.access_token = token.access_token as string;
      }
      return session;
    },
  },
});
