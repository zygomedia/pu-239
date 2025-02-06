use std::collections::BTreeMap;

// use proc_macro2::{TokenStream, Ident, Span};
use syn::{spanned::Spanned, visit::Visit};
use quote::{ToTokens, quote};
// use std::fmt::Write;

// use syn::{visit::Visit, visit_mut::VisitMut};
// use quote::quote;

fn quick_hash<T: std::hash::Hash>(t: &T) -> u64 {
	use std::{collections::hash_map::DefaultHasher, hash::Hasher};

	let mut hasher = DefaultHasher::new();
	t.hash(&mut hasher);
	hasher.finish()
}

#[proc_macro_attribute]
pub fn server(_: proc_macro::TokenStream, item: proc_macro::TokenStream) -> proc_macro::TokenStream {
	let mut item = syn::parse_macro_input!(item as syn::ItemFn);
	let hash = quick_hash(&item);
	let output = match item.sig.output {
		syn::ReturnType::Default => syn::parse_quote!(()),
		syn::ReturnType::Type(_, ty) => *ty,
	};
	item.sig.output = syn::parse_quote!(-> ::std::result::Result<#output, ::anyhow::Error>);
	let arg_idents = item.sig.inputs.iter().map(|x| match x {
		syn::FnArg::Typed(x) => x.pat.clone(),
		syn::FnArg::Receiver(_) => panic!("Expected typed argument"),
	});
	item.block = syn::parse_quote!({
		const HASH: u64 = #hash;

		let args = (#(#arg_idents),*);
		let mut serialized = ::std::vec::Vec::with_capacity(::postcard::experimental::serialized_size(&HASH)? + ::postcard::experimental::serialized_size(&args)?);
		::postcard::to_io(&HASH, &mut serialized)?;
		::postcard::to_io(&args, &mut serialized)?;
		Ok(::postcard::from_bytes(&crate::api::dispatch(serialized).await?)?)
	});
	item.into_token_stream().into()
}


struct Visitor<'a> {
	root: &'a std::path::Path,

	api_fns: Vec<syn::ItemFn>,

	current_path: (Vec<syn::Ident>, Vec<syn::Attribute>),
	sub_visitors: BTreeMap<syn::Ident, Self>,
}

impl<'a> Visitor<'a> {
	fn new(root: &'a std::path::Path, current_path: (Vec<syn::Ident>, Vec<syn::Attribute>)) -> Self {
		Self { root, api_fns: Vec::new(), current_path, sub_visitors: BTreeMap::new() }
	}

	fn write_out(&self, out: &mut Vec<syn::Item>) {
		for f in &self.api_fns {
			out.push(syn::parse_quote!(#f));
		}

		for (module, sub_visitor) in &self.sub_visitors {
			if sub_visitor.api_fns.is_empty() { continue; }
			let mut sub_out: Vec<syn::Item> = Vec::with_capacity(sub_visitor.api_fns.len() + sub_visitor.sub_visitors.len());
			sub_visitor.write_out(&mut sub_out);
			out.push(syn::parse_quote!(pub mod #module { #(#sub_out)* }));
		}
	}

	fn write_arms(&self, out: &mut Vec<syn::Arm>) {
		for f in &self.api_fns {
			let hash = quick_hash(&f);
			let current_path = &self.current_path.0;
			let fn_ident = &f.sig.ident;
			let fn_path = quote!(#(#current_path ::)*#fn_ident);
			let arg_idents = &f.sig.inputs.iter().map(|x| match x {
				syn::FnArg::Typed(x) => x.pat.clone(),
				syn::FnArg::Receiver(_) => panic!("Expected typed argument"),
			}).collect::<Vec<_>>();
			#[cfg(feature = "trace")] let fn_path_str = fn_path.to_string().replace(" ", "");
			#[cfg(feature = "trace")] let log_str_pre = format!("{fn_path_str}{{args:?}}");
			#[cfg(feature = "trace")] let log_str_post = format!("{fn_path_str} -> {{res:?}}");
			#[cfg(feature = "trace")] let maybe_trace_pre = quote!(log::trace!(#log_str_pre););
			#[cfg(feature = "trace")] let maybe_trace_post = quote!(log::trace!(#log_str_post););
			#[cfg(not(feature = "trace"))] let maybe_trace_pre = quote!();
			#[cfg(not(feature = "trace"))] let maybe_trace_post = quote!();
			out.push(syn::parse_quote!(#hash => {
				let args = ::postcard::from_io::<_, _>((&mut bytes, &mut scratch))?.0;
				#maybe_trace_pre
				let (#(#arg_idents),*) = args;
				let res = #fn_path(#(#arg_idents),*).await;
				#maybe_trace_post
				Ok(::postcard::to_stdvec(&res)?)
			}));
		}

		for sub_visitor in self.sub_visitors.values() {
			sub_visitor.write_arms(out);
		}
	}

	fn total_fns(&self) -> usize {
		self.api_fns.len() + self.sub_visitors.values().map(Visitor::total_fns).sum::<usize>()
	}
}

impl Visit<'_> for Visitor<'_> {
	// create a visitor for each api module or file, recursive
	fn visit_item_mod(&mut self, node: &syn::ItemMod) {
		let mut path = self.current_path.0.clone();
		path.push(node.ident.clone());
		let mut visitor = Visitor::new(self.root, (path, node.attrs.clone()));
		if let Some((_, items)) = &node.content {
			for item in items {
				visitor.visit_item(item);
			}
		} else {
			let mut path = visitor.root.to_owned();
			for seg in &visitor.current_path.0 {
				path.push(seg.to_string());
			}

			let name_rs = path.with_extension("rs");
			let mod_rs = path.join("mod.rs");

			let path = if name_rs.exists() { name_rs } else if mod_rs.exists() { mod_rs } else {
				panic!("No file found for module {}, is there a loose mod declaration that isn't pointing anywhere?", visitor.current_path.0.last().unwrap())
			};

			let file = std::fs::read_to_string(&path).expect("Error reading file. Is there a loose mod declaration that isn't pointing anywhere?");
			visitor.visit_file(&syn::parse_file(&file).unwrap());
		}

		self.sub_visitors.insert(node.ident.clone(), visitor);
	}

	fn visit_item_fn(&mut self, node: &syn::ItemFn) {
		let pu_239_server: syn::Path = syn::parse_quote!(pu_239::server);
		let Some(_api_attr) = node.attrs.iter().find(|attr| *attr.path() == pu_239_server) else { return; };
		let mut node = node.clone();
		node.attrs.retain(|attr| *attr.path() != pu_239_server);
		self.api_fns.push(node);
	}
}

#[proc_macro]
pub fn build_api(item: proc_macro::TokenStream) -> proc_macro::TokenStream {
	let roots = syn::parse_macro_input!(item as syn::ExprArray).elems.into_iter()
		.map(|elem| {
			let root: syn::LitStr = syn::parse_quote!(#elem);
			std::path::PathBuf::from(root.value())
		})
		.collect::<Vec<_>>();

	let visitors = roots.iter().map(|root| {
		let mut visitor = Visitor::new(root.parent().unwrap(), (Vec::new(), Vec::new()));
		visitor.visit_file(&syn::parse_file(&std::fs::read_to_string(root).unwrap()).unwrap());
		visitor
	}).collect::<Vec<_>>();

	let mut out = Vec::<syn::Item>::with_capacity(visitors.iter().map(|visitor| visitor.api_fns.len() + visitor.sub_visitors.len()).sum());
	let mut arms = Vec::<syn::Arm>::with_capacity(visitors.iter().map(|visitor| visitor.total_fns()).sum());

	for visitor in visitors {
		visitor.write_out(&mut out);
		visitor.write_arms(&mut arms);
	}

	quote!(
		#(#out)*

		async fn deserialize_api_match(mut bytes: impl ::std::io::Read) -> ::std::result::Result<::std::vec::Vec<u8>, ::anyhow::Error> {
			let mut scratch = [0u8; 2048];
			let (hash, (mut bytes, _)) = ::postcard::from_io::<u64, _>((bytes, &mut scratch))?;
			match hash {
				#(#arms),*
				method_id => Err(::anyhow::anyhow!("Unknown method id: {method_id}")),
			}
		}
	).into()
}
